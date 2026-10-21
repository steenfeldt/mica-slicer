// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/PrintConfig.cpp :: PrintConfigDef::init_fff_params
// Changes: Optional Orca batch B wall, bridge or scarf controls; existing Prusa defaults retained.
// Source: prusaslicer@b028299c77 src/libslic3r/PrintConfig.cpp :: PrintConfigDef::init_fff_params
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: get_seam_gap_distance_value, GCodeGenerator::extrude_perimeters
// Changes: Typed seam settings resolved for the first extruder, including non-nil filament overrides; reject invalid numeric values.
// Changes: Engine choices read mica_slicing_rules (mica_core::rules), not gcode_flavor or the presence of Orca keys (docs/slicing-rules.md).
// Changes: One scarf setting on every printer (crate::merged): PrusaSlicer rules take the Orca scarf values PrusaSlicer's scarf has, Orca rules the PrusaSlicer ones translated.

use crate::config::{Config, ConfigValue, Value};
use crate::resolve::ResolveError;
use crate::settings::FloatOrPercent;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SeamPosition {
    #[default]
    Aligned,
    Nearest,
    Rear,
    Random,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScarfPlacement {
    #[default]
    Nowhere,
    Contours,
    Everywhere,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ScarfSettings {
    /// Orca controls are active only when seam_slope_type is explicitly present.
    pub orca: bool,
    pub conditional: bool,
    pub angle_threshold: f64,
    pub overhang_threshold: f64,
    pub speed: FloatOrPercent,
    pub flow_ratio: f64,
    pub steps: usize,
    pub absolute_start_height: Option<f64>,
    pub placement: ScarfPlacement,
    pub only_on_smooth: bool,
    pub length: f64,
    pub entire_loop: bool,
    pub max_segment_length: f64,
    /// Fraction of layer height, in 0..=1 (the profile option is a percent).
    pub start_height: f64,
    pub on_inner_perimeters: bool,
}
impl Default for ScarfSettings {
    fn default() -> Self {
        Self {
            orca: false,
            conditional: false,
            angle_threshold: 155.,
            overhang_threshold: 0.4,
            speed: FloatOrPercent::Percent(100.),
            flow_ratio: 1.,
            steps: 10,
            absolute_start_height: None,
            placement: ScarfPlacement::Nowhere,
            only_on_smooth: true,
            length: 20.0,
            entire_loop: false,
            max_segment_length: 1.0,
            start_height: 0.0,
            on_inner_perimeters: false,
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct SeamSettings {
    pub position: SeamPosition,
    /// Print both contours and holes clockwise instead of the default counter-clockwise.
    pub prefer_clockwise_movements: bool,
    pub staggered_inner_seams: bool,
    /// Percent of nozzle diameter, or mm. Negative values extend beyond the seam.
    pub gap: FloatOrPercent,
    pub scarf: ScarfSettings,
    /// Retraction wiping along a perimeter disables Prusa's inward seam-hiding travel.
    pub wipe: bool,
    /// PrusaSlicer 2.9 parity: decimated-mesh visibility and upstream perimeter rules. Orca
    /// profiles keep Mica's earlier model (full-mesh visibility), as Orca's seam placer differs.
    pub prusa_parity: bool,
}
impl Default for SeamSettings {
    fn default() -> Self {
        Self {
            position: SeamPosition::Aligned,
            prefer_clockwise_movements: false,
            staggered_inner_seams: false,
            gap: FloatOrPercent::Percent(15.0),
            scarf: ScarfSettings::default(),
            wipe: false,
            prusa_parity: true,
        }
    }
}
fn scalar<'a>(c: &'a Config, key: &str) -> Option<&'a Value> {
    match c.get(key)? {
        ConfigValue::Scalar(v) => Some(v),
        ConfigValue::Vector(v) => v.first()?.as_ref(),
    }
}
fn overridden<'a>(c: &'a Config, key: &str) -> Option<&'a Value> {
    scalar(c, &format!("filament_{key}")).or_else(|| scalar(c, key))
}
fn length_value(v: &Value) -> Option<FloatOrPercent> {
    match *v {
        Value::FloatOrPercent { value, percent } => Some(if percent {
            FloatOrPercent::Percent(value)
        } else {
            FloatOrPercent::Mm(value)
        }),
        Value::Percent(v) => Some(FloatOrPercent::Percent(v)),
        Value::Float(v) => Some(FloatOrPercent::Mm(v)),
        _ => None,
    }
}
impl SeamSettings {
    pub fn from_config(c: &Config) -> Result<Self, ResolveError> {
        let bad = |k: &str| ResolveError(format!("invalid seam setting `{k}`"));
        let flag = |k: &str| c.bool(k).ok_or_else(|| bad(k));
        let position = match c.string("seam_position") {
            Some("aligned") => SeamPosition::Aligned,
            Some("nearest") => SeamPosition::Nearest,
            Some("rear") => SeamPosition::Rear,
            Some("random") => SeamPosition::Random,
            _ => return Err(bad("seam_position")),
        };
        let rules = crate::rules::Rules::of(c);
        // One scarf setting on every printer (`crate::merged`). PrusaSlicer rules read
        // PrusaSlicer's options, translated from the Orca keys where those are set; Orca rules
        // read Orca's, translated from PrusaSlicer's where only those are set.
        let prusa = |k: &str| crate::merged::prusa_value(c, k);
        let prusa_flag = |k: &str| match prusa(k) {
            Some(Value::Bool(b)) => Ok(b),
            _ => Err(bad(k)),
        };
        let prusa_number = |k: &str| match prusa(k) {
            Some(Value::Float(v) | Value::Percent(v)) if v.is_finite() => Ok(v),
            Some(Value::Int(v)) => Ok(v as f64),
            _ => Err(bad(k)),
        };
        let placement = match prusa("scarf_seam_placement") {
            Some(Value::Enum(e)) if e == "nowhere" => ScarfPlacement::Nowhere,
            Some(Value::Enum(e)) if e == "contours" => ScarfPlacement::Contours,
            Some(Value::Enum(e)) if e == "everywhere" => ScarfPlacement::Everywhere,
            _ => return Err(bad("scarf_seam_placement")),
        };
        let gap = overridden(c, "seam_gap_distance")
            .and_then(length_value)
            .ok_or_else(|| bad("seam_gap_distance"))?;
        if !gap.resolve(1.0, 1.0).is_finite() {
            return Err(bad("seam_gap_distance"));
        }
        let start_height = prusa_number("scarf_seam_start_height")? / 100.0;
        let length = prusa_number("scarf_seam_length")?;
        let max_segment_length = prusa_number("scarf_seam_max_segment_length")?;
        if length < 0.0 {
            return Err(bad("scarf_seam_length"));
        }
        if max_segment_length <= 0.0 {
            return Err(bad("scarf_seam_max_segment_length"));
        }
        if !(0.0..=1.0).contains(&start_height) {
            return Err(bad("scarf_seam_start_height"));
        }
        let wipe = match overridden(c, "wipe") {
            Some(Value::Bool(v)) => *v,
            _ => return Err(bad("wipe")),
        };
        // PrusaSlicer's scarf has no joint speed, flow ratio or angle/overhang thresholds: under
        // PrusaSlicer rules they keep their neutral values (the GUI hides them).
        let mut scarf = ScarfSettings {
            placement,
            only_on_smooth: prusa_flag("scarf_seam_only_on_smooth")?,
            length,
            entire_loop: prusa_flag("scarf_seam_entire_loop")?,
            max_segment_length,
            start_height,
            on_inner_perimeters: prusa_flag("scarf_seam_on_inner_perimeters")?,
            ..ScarfSettings::default()
        };
        for (key, value, min, max) in [
            (
                "scarf_angle_threshold",
                c.float("scarf_angle_threshold").unwrap_or(155.),
                0.,
                180.,
            ),
            (
                "scarf_overhang_threshold",
                c.float("scarf_overhang_threshold").unwrap_or(40.),
                0.,
                f64::MAX,
            ),
            (
                "scarf_joint_flow_ratio",
                c.float("scarf_joint_flow_ratio").unwrap_or(1.),
                0.,
                2.,
            ),
            (
                "scarf_joint_speed",
                scalar(c, "scarf_joint_speed")
                    .and_then(length_value)
                    .unwrap_or(FloatOrPercent::Percent(100.))
                    .resolve(100., 1.),
                1.,
                f64::MAX,
            ),
        ] {
            if !value.is_finite() || value < min || value > max {
                return Err(bad(key));
            }
        }
        if rules.is_orca() {
            let orca = |k: &str| crate::merged::orca_value(c, k);
            let orca_flag = |k: &str| match orca(k) {
                Some(Value::Bool(b)) => Ok(b),
                None => Ok(false),
                _ => Err(bad(k)),
            };
            let kind = match orca("seam_slope_type") {
                Some(Value::Enum(e)) => e,
                None => "none".into(),
                _ => return Err(bad("seam_slope_type")),
            };
            scarf.orca = true;
            scarf.placement = match kind.as_str() {
                "none" => ScarfPlacement::Nowhere,
                "external" => ScarfPlacement::Contours,
                "all" => ScarfPlacement::Everywhere,
                _ => return Err(bad("seam_slope_type")),
            };
            scarf.conditional = orca_flag("seam_slope_conditional")?;
            scarf.only_on_smooth = false;
            scarf.entire_loop = orca_flag("seam_slope_entire_loop")?;
            scarf.on_inner_perimeters = orca_flag("seam_slope_inner_walls")?;
            scarf.length = match orca("seam_slope_min_length") {
                Some(Value::Float(v)) => v,
                None => 20.,
                _ => return Err(bad("seam_slope_min_length")),
            };
            scarf.angle_threshold = c.float("scarf_angle_threshold").unwrap_or(155.);
            scarf.overhang_threshold = c.float("scarf_overhang_threshold").unwrap_or(40.) / 100.;
            scarf.flow_ratio = c.float("scarf_joint_flow_ratio").unwrap_or(1.);
            scarf.steps = match orca("seam_slope_steps") {
                Some(Value::Int(v)) => v.max(0) as usize,
                None => 10,
                _ => return Err(bad("seam_slope_steps")),
            };
            scarf.speed = scalar(c, "scarf_joint_speed")
                .and_then(length_value)
                .unwrap_or(FloatOrPercent::Percent(100.));
            match orca("seam_slope_start_height")
                .as_ref()
                .and_then(length_value)
            {
                Some(FloatOrPercent::Mm(h)) => {
                    scarf.absolute_start_height = Some(h);
                    scarf.start_height = 0.;
                }
                Some(FloatOrPercent::Percent(h)) => scarf.start_height = h / 100.,
                None => scarf.start_height = 0.,
            }
            for (key, value, min, max) in [
                ("seam_slope_min_length", scarf.length, 0., f64::MAX),
                (
                    "seam_slope_start_height",
                    scarf.absolute_start_height.unwrap_or(scarf.start_height),
                    0.,
                    f64::MAX,
                ),
            ] {
                if !value.is_finite() || value < min || value > max {
                    return Err(bad(key));
                }
            }
            if scarf.steps == 0 {
                return Err(bad("seam_slope_steps"));
            }
        }
        Ok(Self {
            position,
            prefer_clockwise_movements: flag("prefer_clockwise_movements")?,
            staggered_inner_seams: flag("staggered_inner_seams")?,
            gap,
            wipe,
            prusa_parity: !scarf.orca,
            scarf,
        })
    }
}
