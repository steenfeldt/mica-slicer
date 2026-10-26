// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/PrintConfig.cpp :: PrintConfigDef::init_fff_params
// Source: prusaslicer@b028299c77 src/libslic3r/Flow.cpp :: Flow::extrusion_width, Flow::auto_extrusion_width, support_material_flow, support_material_1st_layer_flow
// Source: prusaslicer@b028299c77 src/libslic3r/PrintConfig.cpp :: PrintConfigDef::init_fff_params, DynamicPrintConfig::normalize_fdm
// Source: prusaslicer@b028299c77 src/libslic3r/PlaceholderParser.cpp :: MyContext::scalar_variable_reference
// Source: orca@8500fcdcca src/libslic3r/PerimeterGenerator.cpp :: PerimeterGenerator::process_arachne
// Source: orca@8500fcdcca src/libslic3r/Fill/Fill.cpp :: Layer::make_fills
// Source: orca@8500fcdcca src/libslic3r/LayerRegion.cpp :: LayerRegion::process_external_surfaces
// Source: orca@8500fcdcca src/libslic3r/Support/SupportMaterial.cpp :: PrintObjectSupportMaterial::raft_and_intermediate_support_layers
// Changes: Orca's support layers follow the object layers when independent_support_layer_height is off.
// Changes: Resolve Orca single-wall top filtering from its source options, independently of printer flavor.
// Changes: Width fallbacks (0 → extrusion_width → automatic), percentages of layer height and the speed
// Changes: percentage bases follow PrusaSlicer; resolved once for the first extruder into PrintSettings.
// Changes: absolute_value: a percentage option's absolute value with the engine's arithmetic (for Orca files).
// Changes: The wall order is the merged wall sequence on every printer (crate::merged).
// Changes: Engine choices read mica_slicing_rules (mica_core::rules), not gcode_flavor or the presence of Orca keys (docs/slicing-rules.md).

//! Building engine settings from a resolved configuration.

use std::fmt;

use crate::arachne::ArachneSettings;
use crate::config::{Config, ConfigValue, Value};
use crate::fill::Pattern;
use crate::settings::{
    EnsureVerticalShellThickness, FloatOrPercent, PerimeterGenerator, PrintSettings,
    TopOnePerimeter,
};

#[derive(Debug, Clone, PartialEq)]
pub struct ResolveError(pub String);

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ResolveError {}

fn missing(key: &str) -> ResolveError {
    ResolveError(format!("setting `{key}` is missing or has the wrong type"))
}

// Orca source keys retained by the importer also accept text overrides from the CLI.
fn source_bool(c: &Config, key: &str) -> Result<bool, ResolveError> {
    match c.extra(key) {
        Some("1" | "true") => Ok(true),
        Some("0" | "false") => Ok(false),
        Some(_) => Err(missing(key)),
        None => Ok(c.bool(key).unwrap_or(false)),
    }
}

/// Value of a float-or-percent option as written.
fn float_or_percent(c: &Config, key: &str) -> Result<FloatOrPercent, ResolveError> {
    if let Some(raw) = c.extra(key) {
        let value: f64 = raw
            .trim_end_matches('%')
            .parse()
            .map_err(|_| missing(key))?;
        if !value.is_finite() {
            return Err(missing(key));
        }
        return Ok(if raw.ends_with('%') {
            FloatOrPercent::Percent(value)
        } else {
            FloatOrPercent::Mm(value)
        });
    }
    match c.get(key) {
        Some(ConfigValue::Scalar(Value::FloatOrPercent {
            value,
            percent: true,
        })) => Ok(FloatOrPercent::Percent(*value)),
        Some(ConfigValue::Scalar(
            Value::FloatOrPercent {
                value,
                percent: false,
            }
            | Value::Float(value),
        )) => Ok(FloatOrPercent::Mm(*value)),
        Some(ConfigValue::Scalar(Value::Percent(p))) => Ok(FloatOrPercent::Percent(*p)),
        _ => Err(missing(key)),
    }
}

fn pattern(c: &Config, key: &str) -> Result<Pattern, ResolveError> {
    let name = c.string(key).ok_or_else(|| missing(key))?;
    Pattern::from_config_name(name)
        .ok_or_else(|| ResolveError(format!("{key} = {name}: this pattern is not supported yet")))
}

fn float(c: &Config, key: &str) -> Result<f64, ResolveError> {
    c.float(key).ok_or_else(|| missing(key))
}

fn int(c: &Config, key: &str) -> Result<i64, ResolveError> {
    c.int(key).ok_or_else(|| missing(key))
}

/// Per-extruder value for the first extruder, preferring a non-nil filament override `filament_<key>`.
fn extruder_float(c: &Config, key: &str) -> Result<f64, ResolveError> {
    if let Some(ConfigValue::Vector(items)) = c.get(&format!("filament_{key}"))
        && let Some(Some(_)) = items.first()
    {
        return c
            .float(&format!("filament_{key}"))
            .ok_or_else(|| missing(key));
    }
    float(c, key)
}

/// Absolute speed: percentages are of `base`.
fn speed(c: &Config, key: &str, base: f64) -> Result<f64, ResolveError> {
    Ok(float_or_percent(c, key)?.resolve(base, 1.0))
}

/// PrusaSlicer's automatic width for a role: 1.125 × nozzle, or the nozzle diameter for top infill.
fn auto_width(key: &str, nozzle: f64) -> f64 {
    if matches!(
        key,
        "top_infill_extrusion_width" | "support_material_extrusion_width"
    ) {
        nozzle
    } else {
        1.125 * nozzle
    }
}

/// Extrusion width in mm following `Flow::extrusion_width`: 0 falls back to `extrusion_width`,
/// percentages are of the (first) layer height, and a remaining 0 means automatic.
fn extrusion_width(
    c: &Config,
    key: &str,
    layer_height: f64,
    first_layer_height: f64,
) -> Result<f64, ResolveError> {
    let mut first_layer = key.starts_with("first_layer_");
    let mut opt = float_or_percent(c, key)?;
    if matches!(opt, FloatOrPercent::Mm(v) | FloatOrPercent::Percent(v) if v == 0.0) {
        opt = float_or_percent(c, "extrusion_width")?;
        first_layer = false;
    }
    let width = match opt {
        FloatOrPercent::Percent(_) => opt.resolve(
            if first_layer {
                first_layer_height
            } else {
                layer_height
            },
            1.0,
        ),
        FloatOrPercent::Mm(v) => v,
    };
    if width == 0.0 {
        let nozzle = float(c, "nozzle_diameter")?;
        // first_layer_extrusion_width = 0 → automatic like a perimeter.
        return Ok(auto_width(key, nozzle));
    }
    Ok(width)
}

/// Absolute value of a float-or-percent option the way PrusaSlicer's template language reads it:
/// extrusion widths through the width rules, other percentages through their `ratio_over` chain.
pub fn float_or_percent_value(c: &Config, key: &str) -> Result<f64, ResolveError> {
    if key.ends_with("extrusion_width") {
        let layer_height = float(c, "layer_height")?;
        let first_layer_height =
            float_or_percent(c, "first_layer_height")?.resolve(layer_height, 1.0);
        return extrusion_width(c, key, layer_height, first_layer_height);
    }
    let FloatOrPercent::Percent(p) = float_or_percent(c, key)? else {
        return float(c, key);
    };
    let mut v = p * 0.01;
    let mut def_key = key.to_string();
    loop {
        let parent = crate::config::option_defs()
            .get(def_key.as_str())
            .and_then(|d| d.ratio_over)
            .ok_or_else(|| {
                ResolveError(format!(
                    "`{key}` is a percentage of nothing (no ratio_over)"
                ))
            })?;
        if parent.ends_with("extrusion_width") {
            return Ok(v * float_or_percent_value(c, parent)?);
        }
        match float_or_percent(c, parent) {
            Ok(FloatOrPercent::Mm(x)) => return Ok(v * x),
            Ok(FloatOrPercent::Percent(x)) => v *= x * 0.01,
            Err(_) => return Ok(v * float(c, parent)?),
        }
        def_key = parent.to_string();
    }
}

/// The absolute value the engine slices a float-or-percent option with (`base × p / 100`,
/// `base` being its `ratio_over` option, itself resolved; widths through the width rules), for
/// files that cannot hold the percentage (OrcaSlicer has only mm/s for several of PrusaSlicer's
/// "mm/s or %" options). The same arithmetic as [`PrintSettings::from_config`], so the value
/// slices identically.
pub fn absolute_value(c: &Config, key: &str) -> Result<f64, ResolveError> {
    if key.ends_with("extrusion_width") {
        return float_or_percent_value(c, key);
    }
    match float_or_percent(c, key)? {
        FloatOrPercent::Mm(v) => Ok(v),
        FloatOrPercent::Percent(p) => {
            let parent = crate::config::option_defs()
                .get(key)
                .and_then(|d| d.ratio_over)
                .ok_or_else(|| {
                    ResolveError(format!(
                        "`{key}` is a percentage of nothing (no ratio_over)"
                    ))
                })?;
            Ok(FloatOrPercent::Percent(p).resolve(absolute_value(c, parent)?, 1.0))
        }
    }
}

impl PrintSettings {
    /// Resolves engine settings for the first extruder from a full configuration (PrusaSlicer names).
    pub fn from_config(c: &Config) -> Result<Self, ResolveError> {
        // PrusaSlicer or OrcaSlicer semantics: one switch, never the presence of an option.
        let rules = crate::rules::Rules::of(c);
        for key in ["top_fill_pattern", "bottom_fill_pattern"] {
            let p = pattern(c, key)?;
            if !p.is_surface_pattern() {
                return Err(ResolveError(format!(
                    "{key} requires a solid surface pattern"
                )));
            }
        }
        let solid =
            crate::fill::config_pattern(c, "internal_solid_infill_pattern", Pattern::Ensuring)?;
        if solid != Pattern::Ensuring && !solid.is_surface_pattern() {
            return Err(ResolveError(
                "internal_solid_infill_pattern requires a solid surface pattern".into(),
            ));
        }
        if pattern(c, "fill_pattern")? == Pattern::Ensuring {
            return Err(ResolveError("ensuring is an internal solid pattern".into()));
        }
        for (key, default, min, max) in [
            ("internal_bridge_angle", 0., 0., 180.),
            ("bridge_density", 100., 10., 125.),
            ("internal_bridge_density", 100., 10., 125.),
        ] {
            let value = c.float(key).unwrap_or(default);
            if !value.is_finite() || !(min..=max).contains(&value) {
                return Err(ResolveError(format!("invalid {key}")));
            }
        }
        let bridge_flow_ratio = float(c, "bridge_flow_ratio")?;
        if !bridge_flow_ratio.is_finite() || bridge_flow_ratio <= 0.0 {
            return Err(ResolveError(
                "bridge_flow_ratio must be finite and positive".into(),
            ));
        }
        let layer_height = float(c, "layer_height")?;
        let first_layer_height =
            float_or_percent(c, "first_layer_height")?.resolve(layer_height, 1.0);
        if !layer_height.is_finite()
            || layer_height <= 0.0
            || !first_layer_height.is_finite()
            || first_layer_height <= 0.0
        {
            return Err(ResolveError(
                "layer heights must be finite and positive".into(),
            ));
        }
        let nozzle = float(c, "nozzle_diameter")?;
        if !nozzle.is_finite()
            || nozzle <= 0.
            || layer_height > nozzle
            || first_layer_height > nozzle
        {
            return Err(ResolveError(
                "layer height exceeds nozzle diameter or nozzle is invalid".into(),
            ));
        }
        let width = |key: &str| extrusion_width(c, key, layer_height, first_layer_height);

        let perimeter_speed = float(c, "perimeter_speed")?;
        let infill_speed = float(c, "infill_speed")?;
        let solid_infill_speed = speed(c, "solid_infill_speed", infill_speed)?;
        let first_layer_speed = match float_or_percent(c, "first_layer_speed")? {
            FloatOrPercent::Mm(v) => v,
            FloatOrPercent::Percent(_) => {
                float_or_percent(c, "first_layer_speed")?.resolve(perimeter_speed, 1.0)
            }
        };

        let default_acceleration = float(c, "default_acceleration")?;
        // 0 means "leave the printer's acceleration alone"; Mica then uses the default acceleration.
        let accel = |key: &str| -> Result<f64, ResolveError> {
            let a = float(c, key)?;
            Ok(if a > 0.0 { a } else { default_acceleration })
        };

        let print_max_volumetric = float(c, "max_volumetric_speed")?;
        let filament_max_volumetric = float(c, "filament_max_volumetric_speed")?;
        let max_volumetric = match (print_max_volumetric > 0.0, filament_max_volumetric > 0.0) {
            (true, true) => print_max_volumetric.min(filament_max_volumetric),
            (true, false) => print_max_volumetric,
            (false, true) => filament_max_volumetric,
            (false, false) => f64::INFINITY,
        };

        let retract_speed = extruder_float(c, "retract_speed")?;
        let deretract_speed = extruder_float(c, "deretract_speed")?;
        let idle_temperature = match c.get("idle_temperature") {
            Some(ConfigValue::Vector(items)) => {
                items.first().cloned().flatten().and_then(|v| match v {
                    Value::Int(i) => Some(i as f64),
                    Value::Float(f) => Some(f),
                    _ => None,
                })
            }
            _ => None,
        };

        let arachne = ArachneSettings {
            wall_transition_length: float_or_percent(c, "wall_transition_length")?,
            wall_transition_filter_deviation: float_or_percent(
                c,
                "wall_transition_filter_deviation",
            )?,
            wall_transition_angle: float(c, "wall_transition_angle")?,
            wall_distribution_count: int(c, "wall_distribution_count")?.max(0) as usize,
            min_feature_size: float_or_percent(c, "min_feature_size")?,
            min_bead_width: float_or_percent(c, "min_bead_width")?,
        };
        arachne
            .validate()
            .map_err(|key| ResolveError(format!("setting `{key}` is outside its Arachne range")))?;
        let mut adhesion_width = 0.0;
        for key in [
            "first_layer_extrusion_width",
            "perimeter_extrusion_width",
            "extrusion_width",
        ] {
            adhesion_width = float_or_percent(c, key)?.resolve(first_layer_height, 1.0);
            if adhesion_width > 0.0 {
                break;
            }
        }
        if adhesion_width == 0.0 {
            adhesion_width = 1.125 * float(c, "nozzle_diameter")?;
        }
        let mut resolved = Self {
            adhesion: crate::brim::AdhesionSettings::from_config(c, adhesion_width)?,
            layer_height,
            first_layer_height,
            complete_layer_heights: rules.is_orca(),
            elefant_foot_compensation: float(c, "elefant_foot_compensation")?,
            xy_size_compensation: float(c, "xy_size_compensation")?,
            small_features: crate::small_features::SmallFeatureSettings::from_config(c)?,
            raft_layers: int(c, "raft_layers")?.max(0) as usize,
            support_material: c
                .bool("support_material")
                .ok_or_else(|| missing("support_material"))?,
            support_material_auto: c
                .bool("support_material_auto")
                .ok_or_else(|| missing("support_material_auto"))?,
            support_material_threshold: float(c, "support_material_threshold")?,
            tree3d: None,
            support_material_style: crate::support::SupportStyle::parse(
                c.string("support_material_style")
                    .ok_or_else(|| missing("support_material_style"))?,
            )
            .ok_or_else(|| missing("support_material_style"))?,
            support_tree_angle: float(c, "support_tree_angle")?,
            support_tree_angle_slow: float(c, "support_tree_angle_slow")?,
            support_tree_branch_diameter: float(c, "support_tree_branch_diameter")?,
            support_tree_branch_diameter_angle: float(c, "support_tree_branch_diameter_angle")?,
            support_tree_branch_diameter_double_wall: float(
                c,
                "support_tree_branch_diameter_double_wall",
            )?,
            support_tree_branch_distance: float(c, "support_tree_branch_distance")?,
            support_tree_tip_diameter: float(c, "support_tree_tip_diameter")?,
            support_tree_top_rate: float(c, "support_tree_top_rate")? / 100.0,
            support_material_contact_distance: float(c, "support_material_contact_distance")?,
            support_material_bottom_contact_distance: float(
                c,
                "support_material_bottom_contact_distance",
            )?,
            support_material_xy_spacing: float_or_percent(c, "support_material_xy_spacing")?,
            support_material_spacing: float(c, "support_material_spacing")?,
            support_material_angle: float(c, "support_material_angle")?,
            support_material_interface_layers: int(c, "support_material_interface_layers")?.max(0)
                as usize,
            support_material_bottom_interface_layers: int(
                c,
                "support_material_bottom_interface_layers",
            )? as i32,
            support_material_interface_spacing: float(c, "support_material_interface_spacing")?,
            support_material_pattern: crate::support::SupportPattern::parse(
                c.string("support_material_pattern")
                    .ok_or_else(|| missing("support_material_pattern"))?,
            )
            .ok_or_else(|| missing("support_material_pattern"))?,
            support_material_interface_pattern: crate::support::InterfacePattern::parse(
                c.string("support_material_interface_pattern")
                    .ok_or_else(|| missing("support_material_interface_pattern"))?,
            )
            .ok_or_else(|| missing("support_material_interface_pattern"))?,
            support_material_buildplate_only: c
                .bool("support_material_buildplate_only")
                .ok_or_else(|| missing("support_material_buildplate_only"))?,
            support_material_closing_radius: float(c, "support_material_closing_radius")?,
            support_material_with_sheath: c
                .bool("support_material_with_sheath")
                .ok_or_else(|| missing("support_material_with_sheath"))?,
            support_remove_small_overhang: c.bool("support_remove_small_overhang").unwrap_or(false),
            support_orca_semantics: rules.is_orca(),
            // Orca's support layers follow the object layers unless they have an independent
            // layer height (a prime tower with several filaments turns that off, see
            // `input::build::normalize_orca_tower`).
            support_material_synchronize_layers: if rules.is_orca() {
                c.raw("independent_support_layer_height")
                    .is_some_and(|v| matches!(v.trim(), "0" | "false"))
            } else {
                c.bool("support_material_synchronize_layers")
                    .ok_or_else(|| missing("support_material_synchronize_layers"))?
            },
            support_material_enforce_layers: int(c, "support_material_enforce_layers")?.max(0)
                as usize,
            dont_support_bridges: c
                .bool("dont_support_bridges")
                .ok_or_else(|| missing("dont_support_bridges"))?,
            raft_contact_distance: float(c, "raft_contact_distance")?,
            raft_expansion: float(c, "raft_expansion")?,
            raft_first_layer_density: float(c, "raft_first_layer_density")? / 100.0,
            raft_first_layer_expansion: float(c, "raft_first_layer_expansion")?,
            support_material_extrusion_width: width("support_material_extrusion_width")?,
            support_material_first_layer_extrusion_width: if float(
                c,
                "first_layer_extrusion_width",
            )? > 0.0
            {
                width("first_layer_extrusion_width")?
            } else {
                extrusion_width(
                    c,
                    "support_material_extrusion_width",
                    first_layer_height,
                    first_layer_height,
                )?
            },
            support_material_interface_contact_loops: c
                .bool("support_material_interface_contact_loops")
                .ok_or_else(|| missing("support_material_interface_contact_loops"))?,
            support_material_speed: float(c, "support_material_speed")?,
            support_material_interface_speed: speed(
                c,
                "support_material_interface_speed",
                float(c, "support_material_speed")?,
            )?,
            min_layer_height: float(c, "min_layer_height")?,
            max_layer_height: float(c, "max_layer_height")?,
            perimeter_generator: match c.string("perimeter_generator") {
                Some("classic") => PerimeterGenerator::Classic,
                Some("arachne") => PerimeterGenerator::Arachne,
                _ => return Err(missing("perimeter_generator")),
            },
            arachne,
            top_one_perimeter: match c.string("top_one_perimeter_type") {
                Some("none") => TopOnePerimeter::None,
                Some("top") => TopOnePerimeter::TopSurfaces,
                Some("topmost") => TopOnePerimeter::TopmostOnly,
                _ => return Err(missing("top_one_perimeter_type")),
            },
            top_surface_min_width: c
                .float("min_width_top_surface")
                .map(FloatOrPercent::Mm)
                .unwrap_or(FloatOrPercent::Percent(100.)),
            top_surface_wall_extension: if rules.is_orca() { 0.85 } else { 1.0 },
            only_one_perimeter_first_layer: c
                .bool("only_one_perimeter_first_layer")
                .ok_or_else(|| missing("only_one_perimeter_first_layer"))?,
            overhangs: c.bool("overhangs").ok_or_else(|| missing("overhangs"))?,
            supports_touching: (c.bool("support_material").unwrap_or(false)
                || int(c, "support_material_enforce_layers")? > 0)
                && float(c, "support_material_contact_distance")? == 0.0,
            bridge_angle: float(c, "bridge_angle")?,
            bridge_fan_speed: extruder_float(c, "bridge_fan_speed")?,
            bridge_flow_ratio,
            thick_bridges: c
                .bool("thick_bridges")
                .ok_or_else(|| missing("thick_bridges"))?,
            thick_internal_bridges: c.bool("thick_internal_bridges").unwrap_or(true),
            bridge_density: c.abs_value("bridge_density", 1.).unwrap_or(1.),
            internal_bridge_density: c.abs_value("internal_bridge_density", 1.).unwrap_or(1.),
            bridge_style: if rules.is_orca() {
                crate::bridge::BridgeStyle::Orca
            } else {
                crate::bridge::BridgeStyle::Prusa
            },
            bridge_speed: float(c, "bridge_speed")?,
            bridge_acceleration: accel("bridge_acceleration")?,
            seam: crate::seam::SeamSettings::from_config(c)?,
            perimeters: int(c, "perimeters")?.max(0) as usize,
            gcode_resolution: float(c, "gcode_resolution")?
                .max(crate::settings::MIN_GCODE_RESOLUTION),
            slice_closing_radius: crate::slice::closing_radius(c),
            // One wall order on every printer (`crate::merged`): Orca's wall sequence, else
            // PrusaSlicer's external perimeters first.
            external_perimeters_first: match crate::merged::orca_raw(c, "wall_sequence").as_deref()
            {
                Some("outer wall/inner wall") => true,
                Some("inner wall/outer wall" | "inner-outer-inner wall") => false,
                None => return Err(missing("external_perimeters_first")),
                _ => return Err(ResolveError("invalid wall_sequence".into())),
            },
            infill_first: c.bool("infill_first").unwrap_or(false),
            orca_walls: crate::wall_options::WallOptions::from_config(c)?,
            extra_bridge_layer: crate::bridge::ExtraBridgeLayer::from_config(c)?,
            internal_bridge_angle: c.float("internal_bridge_angle").unwrap_or(0.),
            precise_outer_wall: source_bool(c, "precise_outer_wall")?
                && c.string("wall_sequence")
                    .or_else(|| c.extra("wall_sequence"))
                    == Some("inner wall/outer wall"),
            gap_fill_enabled: c
                .bool("gap_fill_enabled")
                .ok_or_else(|| missing("gap_fill_enabled"))?,
            thin_walls: c.bool("thin_walls").ok_or_else(|| missing("thin_walls"))?,
            spiral_vase: crate::spiral_vase::enabled(c),
            top_solid_layers: int(c, "top_solid_layers")?.max(0) as usize,
            bottom_solid_layers: int(c, "bottom_solid_layers")?.max(0) as usize,
            top_solid_min_thickness: float(c, "top_solid_min_thickness")?,
            bottom_solid_min_thickness: float(c, "bottom_solid_min_thickness")?,
            ensure_vertical_shell_thickness: {
                let key = "ensure_vertical_shell_thickness";
                let name = c.string(key).ok_or_else(|| missing(key))?;
                EnsureVerticalShellThickness::from_config_name(name)
                    .ok_or_else(|| ResolveError(format!("{key} = {name}: unknown value")))?
            },
            solid_infill_below_area: float(c, "solid_infill_below_area")?,
            minimum_sparse_infill_area: c.float("minimum_sparse_infill_area"),
            infill_every_layers: int(c, "infill_every_layers")?.max(1) as usize,
            solid_infill_every_layers: int(c, "solid_infill_every_layers")?.max(0) as usize,
            fill_density: float(c, "fill_density")? / 100.0,
            fill_angle: float(c, "fill_angle")?,
            infill_overlap: float_or_percent(c, "infill_overlap")?,
            // Orca's default (25%) when an Orca-rules config lacks the option.
            top_bottom_infill_overlap: rules
                .is_orca()
                .then(|| {
                    if c.get("top_bottom_infill_wall_overlap").is_some()
                        || c.extra("top_bottom_infill_wall_overlap").is_some()
                    {
                        float_or_percent(c, "top_bottom_infill_wall_overlap")
                    } else {
                        Ok(FloatOrPercent::Percent(25.))
                    }
                })
                .transpose()?,
            fill_pattern: pattern(c, "fill_pattern")?,
            zag: crate::fill::ZagSettings::from_config(c)?,
            top_fill_pattern: pattern(c, "top_fill_pattern")?,
            bottom_fill_pattern: pattern(c, "bottom_fill_pattern")?,
            internal_solid_fill_pattern: solid,
            detect_narrow_internal_solid_infill: source_bool(
                c,
                "detect_narrow_internal_solid_infill",
            )?,
            ironing: crate::ironing::IroningSettings::from_config(c)?,
            fuzzy_skin: crate::fuzzy_skin::FuzzySkinSettings::from_config(c)?,
            texture_skin: crate::texture::TextureSettings::from_config(c)?,
            textured_mesh: false,
            brick: crate::brick_layers::BrickSettings::from_config(c)?,
            infill_anchor: float_or_percent(c, "infill_anchor")?,
            infill_anchor_max: float_or_percent(c, "infill_anchor_max")?,
            nozzle_diameter: float(c, "nozzle_diameter")?,
            first_layer_extrusion_width: width("first_layer_extrusion_width")?,
            external_perimeter_extrusion_width: width("external_perimeter_extrusion_width")?,
            perimeter_extrusion_width: width("perimeter_extrusion_width")?,
            infill_extrusion_width: width("infill_extrusion_width")?,
            solid_infill_extrusion_width: width("solid_infill_extrusion_width")?,
            top_infill_extrusion_width: width("top_infill_extrusion_width")?,
            first_layer_speed,
            external_perimeter_speed: speed(c, "external_perimeter_speed", perimeter_speed)?,
            perimeter_speed,
            infill_speed,
            solid_infill_speed,
            top_solid_infill_speed: speed(c, "top_solid_infill_speed", solid_infill_speed)?,
            gap_fill_speed: float(c, "gap_fill_speed")?,
            travel_speed: float(c, "travel_speed")?,
            travel_speed_z: float(c, "travel_speed_z")?,
            default_acceleration,
            first_layer_acceleration: accel("first_layer_acceleration")?,
            external_perimeter_acceleration: accel("external_perimeter_acceleration")?,
            perimeter_acceleration: accel("perimeter_acceleration")?,
            infill_acceleration: accel("infill_acceleration")?,
            solid_infill_acceleration: accel("solid_infill_acceleration")?,
            top_solid_infill_acceleration: accel("top_solid_infill_acceleration")?,
            filament_diameter: float(c, "filament_diameter")?,
            extrusion_multiplier: float(c, "extrusion_multiplier")?,
            filament_max_volumetric_speed: max_volumetric,
            filament_type: c
                .string("filament_type")
                .ok_or_else(|| missing("filament_type"))?
                .to_string(),
            first_layer_temperature: float(c, "first_layer_temperature")?,
            temperature: float(c, "temperature")?,
            first_layer_bed_temperature: float(c, "first_layer_bed_temperature")?,
            bed_temperature: float(c, "bed_temperature")?,
            chamber_temperature: float(c, "chamber_temperature")?,
            // Prusa profiles set pressure advance in the filament start G-code (M572), which the
            // template interpreter evaluates; there is no separate setting for it.
            pressure_advance: 0.0,
            idle_temperature,
            filament_abrasive: c.bool("filament_abrasive").unwrap_or(false),
            nozzle_high_flow: c.bool("nozzle_high_flow").unwrap_or(false),
            retract_length: extruder_float(c, "retract_length")?,
            retract_speed,
            deretract_speed: if deretract_speed > 0.0 {
                deretract_speed
            } else {
                retract_speed
            },
            retract_lift: extruder_float(c, "retract_lift")?,
            retract_before_travel: extruder_float(c, "retract_before_travel")?,
            min_fan_speed: float(c, "min_fan_speed")?,
            max_fan_speed: float(c, "max_fan_speed")?,
            disable_fan_first_layers: int(c, "disable_fan_first_layers")?.max(0) as usize,
            full_fan_speed_layer: int(c, "full_fan_speed_layer")?.max(0) as usize,
            fan_below_layer_time: float(c, "fan_below_layer_time")?,
            slowdown_below_layer_time: float(c, "slowdown_below_layer_time")?,
            min_print_speed: float(c, "min_print_speed")?,
        };
        // Organic trees always use the tree3d port; its settings are resolved here so the
        // GUI and CLI share one reader.
        resolved.tree3d = Some(crate::support::tree3d::Tree3dConfig::from_resolved(
            c, &resolved,
        )?);
        Ok(resolved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outline_compensation_settings() {
        let mut c = Config::defaults();
        c.set_raw("elefant_foot_compensation", "0.2").unwrap();
        c.set_raw("xy_size_compensation", "-0.1").unwrap();
        c.set_raw("raft_layers", "2").unwrap();
        let settings = PrintSettings::from_config(&c).unwrap();
        assert_eq!(settings.elefant_foot_compensation, 0.2);
        assert_eq!(settings.xy_size_compensation, -0.1);
        assert_eq!(settings.raft_layers, 2);
    }

    #[test]
    fn support_width_speed_and_interface_inheritance() {
        let mut c = Config::defaults();
        for (key, value) in [
            ("extrusion_width", "0"),
            ("first_layer_extrusion_width", "0"),
            ("support_material_extrusion_width", "0"),
            ("support_material_speed", "80"),
            ("support_material_interface_speed", "50%"),
            ("support_material_bottom_interface_layers", "-1"),
        ] {
            c.set_raw(key, value).unwrap();
        }
        let s = PrintSettings::from_config(&c).unwrap();
        assert_eq!(s.support_material_extrusion_width, s.nozzle_diameter);
        assert_eq!(
            s.support_material_first_layer_extrusion_width,
            s.nozzle_diameter
        );
        assert_eq!(s.support_material_interface_speed, 40.0);
        let params =
            crate::support::SupportParameters::new(&crate::support::SupportConfig::from(&s))
                .unwrap();
        assert_eq!(
            params.bottom_interface_layers,
            s.support_material_interface_layers
        );
    }

    #[test]
    fn width_rules() {
        let mut c = Config::defaults();
        for (k, v) in [
            ("nozzle_diameter", "0.4"),
            ("layer_height", "0.2"),
            ("extrusion_width", "0"),
            ("perimeter_extrusion_width", "0"),
            ("top_infill_extrusion_width", "0"),
            ("infill_extrusion_width", "225%"),
            ("first_layer_extrusion_width", "250%"),
        ] {
            c.set_raw(k, v).unwrap();
        }
        assert!(
            (extrusion_width(&c, "perimeter_extrusion_width", 0.2, 0.3).unwrap() - 0.45).abs()
                < 1e-12
        );
        assert!(
            (extrusion_width(&c, "top_infill_extrusion_width", 0.2, 0.3).unwrap() - 0.4).abs()
                < 1e-12
        );
        assert!(
            (extrusion_width(&c, "infill_extrusion_width", 0.2, 0.3).unwrap() - 0.45).abs() < 1e-12
        );
        // First-layer percentages are of the first layer height.
        assert!(
            (extrusion_width(&c, "first_layer_extrusion_width", 0.2, 0.3).unwrap() - 0.75).abs()
                < 1e-12
        );
        // A zero width falls back to extrusion_width, now as a normal (not first-layer) width.
        c.set_raw("extrusion_width", "200%").unwrap();
        assert!(
            (extrusion_width(&c, "perimeter_extrusion_width", 0.2, 0.3).unwrap() - 0.4).abs()
                < 1e-12
        );
    }

    #[test]
    fn template_percentages_follow_ratio_over() {
        let mut c = Config::defaults();
        for (k, v) in [
            ("infill_speed", "300"),
            ("solid_infill_speed", "80%"),
            ("top_solid_infill_speed", "50%"),
        ] {
            c.set_raw(k, v).unwrap();
        }
        assert!(
            (float_or_percent_value(&c, "top_solid_infill_speed").unwrap() - 120.0).abs() < 1e-9
        );
        c.set_raw("solid_infill_speed", "250").unwrap();
        assert!(
            (float_or_percent_value(&c, "top_solid_infill_speed").unwrap() - 125.0).abs() < 1e-9
        );
    }

    #[test]
    fn speed_percentages() {
        let mut c = Config::defaults();
        c.set_raw("perimeter_speed", "200").unwrap();
        c.set_raw("external_perimeter_speed", "50%").unwrap();
        assert_eq!(speed(&c, "external_perimeter_speed", 200.0).unwrap(), 100.0);
    }

    #[test]
    fn arachne_selection_and_nozzle_relative_settings() {
        let mut c = Config::defaults();
        c.set_raw("perimeter_generator", "arachne").unwrap();
        c.set_raw("wall_transition_length", "150%").unwrap();
        c.set_raw("min_bead_width", "0.32").unwrap();
        c.set_raw("top_one_perimeter_type", "top").unwrap();
        let s = PrintSettings::from_config(&c).unwrap();
        assert_eq!(s.perimeter_generator, PerimeterGenerator::Arachne);
        assert_eq!(s.arachne.wall_transition_length.resolve(0.4, 1.), 0.6);
        assert_eq!(s.arachne.min_bead_width, FloatOrPercent::Mm(0.32));
        assert_eq!(s.top_one_perimeter, TopOnePerimeter::TopSurfaces);
        c.set_raw("perimeter_generator", "classic").unwrap();
        assert_eq!(
            PrintSettings::from_config(&c).unwrap().perimeter_generator,
            PerimeterGenerator::Classic
        );
    }

    #[test]
    fn rejects_invalid_arachne_parameters_before_slicing() {
        for (key, value) in [
            ("wall_distribution_count", "0"),
            ("wall_transition_angle", "60"),
            ("wall_transition_length", "-1%"),
            ("min_feature_size", "-0.1"),
        ] {
            let mut c = Config::defaults();
            c.set_raw(key, value).unwrap();
            assert!(PrintSettings::from_config(&c).unwrap_err().0.contains(key));
        }
    }

    #[test]
    fn filament_override_wins_when_set() {
        let mut c = Config::defaults();
        c.set_raw("retract_length", "0.8").unwrap();
        assert_eq!(extruder_float(&c, "retract_length").unwrap(), 0.8);
        c.set_raw("filament_retract_length", "0.5").unwrap();
        assert_eq!(extruder_float(&c, "retract_length").unwrap(), 0.5);
        c.set_raw("filament_retract_length", "nil").unwrap();
        assert_eq!(extruder_float(&c, "retract_length").unwrap(), 0.8);
    }
}
