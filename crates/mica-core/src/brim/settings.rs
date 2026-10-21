// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Print.cpp :: Print::brim_flow, Print::skirt_flow
// Source: orca@8500fcdcca src/libslic3r/PrintConfig.cpp :: PrintConfigDef::init
// Changes: Typed adhesion settings, validated once; Orca-only options may also be CLI extras.
// Changes: Engine choices read mica_slicing_rules (mica_core::rules), not gcode_flavor or the presence of Orca keys (docs/slicing-rules.md).

use crate::{config::Config, resolve::ResolveError};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BrimType {
    None,
    #[default]
    Outer,
    Inner,
    OuterAndInner,
    Auto,
    Ears,
    /// Orca's "painted": ears only where the object's hand-placed brim ears are.
    Painted,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DraftShield {
    #[default]
    Disabled,
    Limited,
    Enabled,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BrimSettings {
    pub kind: BrimType,
    pub width: f64,
    pub separation: f64,
    pub ears_max_angle: f64,
    pub ears_detection_length: f64,
}

impl Default for BrimSettings {
    fn default() -> Self {
        Self {
            kind: BrimType::Outer,
            width: 0.0,
            separation: 0.0,
            ears_max_angle: 125.0,
            ears_detection_length: 1.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AdhesionSettings {
    pub brim: BrimSettings,
    pub skirts: usize,
    pub skirt_distance: f64,
    pub skirt_height: usize,
    pub min_skirt_length: f64,
    pub draft_shield: DraftShield,
    pub per_object: bool,
    pub skirt_speed: f64,
    pub skirt_start_angle: f64,
    pub orca: bool,
    /// Orca 2.4.2 can select the elephant-foot-compensated brim outline.
    pub brim_efc: bool,
    /// Resolved first width → perimeter width → global width → automatic.
    pub extrusion_width: f64,
}

impl Default for AdhesionSettings {
    fn default() -> Self {
        Self {
            brim: BrimSettings::default(),
            skirts: 0,
            skirt_distance: 6.0,
            skirt_height: 1,
            min_skirt_length: 0.0,
            draft_shield: DraftShield::Disabled,
            per_object: false,
            skirt_speed: 50.0,
            skirt_start_angle: -135.0,
            orca: false,
            brim_efc: false,
            extrusion_width: 0.5,
        }
    }
}

impl AdhesionSettings {
    pub fn skirt_layers(&self, count: usize) -> usize {
        if self.draft_shield == DraftShield::Enabled {
            count
        } else {
            self.skirt_height.min(count)
        }
    }

    pub(crate) fn from_config(c: &Config, width: f64) -> Result<Self, ResolveError> {
        // Prefer explicit raw extras to typed imported values for CLI overrides of Orca-only keys.
        let text = |key: &str| c.extra(key).or_else(|| c.string(key));
        let number = |key: &str, default: f64| -> Result<f64, ResolveError> {
            let n = if let Some(raw) = c.extra(key) {
                raw.parse::<f64>()
                    .map_err(|_| ResolveError(format!("invalid {key}: {raw}")))?
            } else {
                c.float(key).unwrap_or(default)
            };
            if !n.is_finite() || n < 0.0 {
                return Err(ResolveError(format!(
                    "{key} must be finite and nonnegative"
                )));
            }
            Ok(n)
        };
        let kind = match text("brim_type").unwrap_or("outer_only") {
            "no_brim" => BrimType::None,
            "outer_only" => BrimType::Outer,
            "inner_only" => BrimType::Inner,
            "outer_and_inner" => BrimType::OuterAndInner,
            "auto_brim" => BrimType::Auto,
            "brim_ears" => BrimType::Ears,
            "painted" => BrimType::Painted,
            value => return Err(ResolveError(format!("unsupported brim_type: {value}"))),
        };
        let orca = crate::rules::Rules::of(c).is_orca();
        let angle = c
            .extra("skirt_start_angle")
            .map(|s| s.parse::<f64>())
            .transpose()
            .map_err(|_| ResolveError("invalid skirt_start_angle".into()))?
            .or_else(|| c.float("skirt_start_angle"))
            .unwrap_or(-135.0);
        if !angle.is_finite() || !(-180.0..=180.0).contains(&angle) {
            return Err(ResolveError(
                "skirt_start_angle must be between -180 and 180".into(),
            ));
        }
        let ears_angle = number("brim_ears_max_angle", 125.0)?;
        if ears_angle > 180.0 {
            return Err(ResolveError(
                "brim_ears_max_angle must be at most 180".into(),
            ));
        }
        let count = |key: &str, default: f64| -> Result<usize, ResolveError> {
            let n = number(key, default)?;
            if n.fract() != 0.0 || n > 100_000.0 {
                return Err(ResolveError(format!("invalid layer/loop count: {key}")));
            }
            Ok(n as usize)
        };
        Ok(Self {
            brim: BrimSettings {
                kind,
                width: number("brim_width", 0.0)?,
                separation: number("brim_separation", 0.0)?,
                ears_max_angle: ears_angle,
                ears_detection_length: number("brim_ears_detection_length", 1.0)?,
            },
            skirts: count("skirts", 0.0)?,
            skirt_distance: number("skirt_distance", 6.0)?,
            skirt_height: count("skirt_height", 1.0)?,
            min_skirt_length: number("min_skirt_length", 0.0)?,
            draft_shield: match text("draft_shield").unwrap_or("disabled") {
                "disabled" => DraftShield::Disabled,
                "limited" => DraftShield::Limited,
                "enabled" => DraftShield::Enabled,
                value => return Err(ResolveError(format!("unsupported draft_shield: {value}"))),
            },
            per_object: match text("skirt_type").unwrap_or("combined") {
                "combined" => false,
                "perobject" => true,
                value => return Err(ResolveError(format!("unsupported skirt_type: {value}"))),
            },
            skirt_speed: number("skirt_speed", 50.0)?,
            skirt_start_angle: angle,
            orca,
            brim_efc: c
                .extra("brim_use_efc_outline")
                .map(|v| v == "1")
                .unwrap_or_else(|| c.bool("brim_use_efc_outline").unwrap_or(false)),
            extrusion_width: width,
        })
    }
}
