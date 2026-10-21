// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/PrintConfig.cpp :: PrintConfigDef::init_fff_params
// Source: prusaslicer@b028299c77 src/libslic3r/PrintConfig.cpp :: PrintConfigDef::init_fff_params
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/LabelObjects.cpp :: LabelObjects::start_object, LabelObjects::all_objects_header
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::process_layer
// Changes: Value translations between the PrusaSlicer and OrcaSlicer spellings of the settings Mica shows once (wall order, scarf seams, object labels); no upstream code.

//! Settings Mica shows **once** although PrusaSlicer and OrcaSlicer spell them differently.
//!
//! Each [`Pair`] has an Orca key, which is the one Mica shows and edits on every printer, and a
//! PrusaSlicer counterpart, which a PrusaSlicer file (INI, 3MF) carries. The Orca key is
//! optional in a configuration (`orca_options_optional.tsv`): a PrusaSlicer profile has only the
//! counterpart, so the Orca value is translated from it ([`orca_raw`]). When both are set the
//! Orca key wins ([`prusa_raw`] translates it back for PrusaSlicer's engine and files), and
//! setting the counterpart alone (`Config::set_raw`, a PrusaSlicer layer) drops the Orca key, so
//! a value PrusaSlicer edited is never shadowed. See docs/setting-names.md.
//!
//! [`PRUSA_KEYED`] lists the merged settings whose Mica key is PrusaSlicer's instead (object
//! labels, one wall on top surfaces): only the Orca files translate.

use crate::config::{Config, ConfigValue, Value, option_defs, parse_value};
use std::collections::BTreeMap;

/// One merged setting: the Orca key Mica shows and its PrusaSlicer counterpart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pair {
    pub orca: &'static str,
    pub prusa: &'static str,
    /// How the values correspond, and what does not survive a translation.
    pub note: &'static str,
}

pub const PAIRS: &[Pair] = &[
    Pair {
        orca: "wall_sequence",
        prusa: "external_perimeters_first",
        note: "Outer/Inner = external perimeters first on, Inner/Outer = off. PrusaSlicer has no \
               Inner/Outer/Inner: it is written as off (lossy).",
    },
    Pair {
        orca: "seam_slope_type",
        prusa: "scarf_seam_placement",
        note: "None = Nowhere, Contour = Contours, Contour and hole = Everywhere.",
    },
    Pair {
        orca: "seam_slope_conditional",
        prusa: "scarf_seam_only_on_smooth",
        note: "Same intent; PrusaSlicer rules use PrusaSlicer's smoothness test, OrcaSlicer rules \
               the angle and overhang thresholds. PrusaSlicer's default is on, Orca's off.",
    },
    Pair {
        orca: "seam_slope_entire_loop",
        prusa: "scarf_seam_entire_loop",
        note: "Identical.",
    },
    Pair {
        orca: "seam_slope_inner_walls",
        prusa: "scarf_seam_on_inner_perimeters",
        note: "Identical.",
    },
    Pair {
        orca: "seam_slope_min_length",
        prusa: "scarf_seam_length",
        note: "Identical (mm; 0 disables the scarf).",
    },
    Pair {
        orca: "seam_slope_start_height",
        prusa: "scarf_seam_start_height",
        note: "A percentage is the same. An Orca height in mm becomes a percentage of the layer \
               height (lossy with variable layer heights).",
    },
    Pair {
        orca: "seam_slope_steps",
        prusa: "scarf_seam_max_segment_length",
        note: "Orca counts segments, PrusaSlicer caps their length: segment length = scarf length \
               / steps (at least 0.15 mm), steps = scarf length / segment length rounded up \
               (lossy).",
    },
];

/// Merged settings whose Mica key is PrusaSlicer's: `(Mica key, Orca keys, note)`.
pub const PRUSA_KEYED: &[(&str, &[&str], &str)] = &[
    (
        "gcode_label_objects",
        &["gcode_label_objects", "exclude_object"],
        "Firmware-specific = Orca's Exclude objects (M486 on Marlin, EXCLUDE_OBJECT on Klipper), \
         OctoPrint comments = Orca's Label objects without Exclude objects. Orca's both-on \
         (comments and firmware labels) becomes Firmware-specific (lossy).",
    ),
    (
        "top_one_perimeter_type",
        &["only_one_wall_top"],
        "Orca's switch is All top surfaces or Disabled; PrusaSlicer's Topmost surface only \
         becomes on (lossy).",
    ),
];

/// The pair whose Orca key is `key`.
pub fn by_orca(key: &str) -> Option<&'static Pair> {
    PAIRS.iter().find(|p| p.orca == key)
}

/// The pair whose PrusaSlicer counterpart is `key`.
pub fn by_prusa(key: &str) -> Option<&'static Pair> {
    PAIRS.iter().find(|p| p.prusa == key)
}

/// PrusaSlicer counterparts Mica never shows: the Orca key stands for them.
pub fn is_counterpart(key: &str) -> bool {
    by_prusa(key).is_some()
}

fn num(v: f64) -> String {
    let r = (v * 1e6).round() / 1e6;
    format!("{}", if r == 0. { 0. } else { r })
}

fn percent_or_mm(raw: &str) -> Option<(f64, bool)> {
    let t = raw.trim();
    match t.strip_suffix('%') {
        Some(p) => p.trim().parse().ok().map(|v| (v, true)),
        None => t.parse().ok().map(|v| (v, false)),
    }
}

fn bool_raw(raw: &str) -> Option<bool> {
    match raw.trim() {
        "1" | "true" => Some(true),
        "0" | "false" => Some(false),
        _ => None,
    }
}

/// Scarf length (mm) the step/segment translation uses: the effective Orca value.
fn scarf_length(c: &Config) -> f64 {
    orca_raw(c, "seam_slope_min_length")
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.)
        .unwrap_or(20.)
}

/// PrusaSlicer spelling of an Orca value (`None`: not a value of that option).
pub fn to_prusa(c: &Config, pair: &Pair, raw: &str) -> Option<String> {
    let raw = raw.trim();
    Some(match pair.orca {
        "wall_sequence" => match raw {
            "outer wall/inner wall" => "1",
            "inner wall/outer wall" | "inner-outer-inner wall" => "0",
            _ => return None,
        }
        .into(),
        "seam_slope_type" => match raw {
            "none" => "nowhere",
            "external" => "contours",
            "all" => "everywhere",
            _ => return None,
        }
        .into(),
        "seam_slope_conditional" | "seam_slope_entire_loop" | "seam_slope_inner_walls" => {
            if bool_raw(raw)? { "1" } else { "0" }.into()
        }
        "seam_slope_min_length" => num(raw.parse::<f64>().ok().filter(|v| v.is_finite())?),
        "seam_slope_start_height" => {
            let (v, percent) = percent_or_mm(raw)?;
            let p = if percent {
                v
            } else {
                let layer = c.float("layer_height").filter(|h| *h > 0.)?;
                v / layer * 100.
            };
            format!("{}%", num(p.clamp(0., 100.)))
        }
        "seam_slope_steps" => {
            let steps = raw.parse::<f64>().ok().filter(|v| *v >= 1.)?;
            num((scarf_length(c) / steps).max(0.15))
        }
        _ => return None,
    })
}

/// OrcaSlicer spelling of a PrusaSlicer value.
pub fn to_orca(c: &Config, pair: &Pair, raw: &str) -> Option<String> {
    let raw = raw.trim();
    Some(match pair.orca {
        "wall_sequence" => if bool_raw(raw)? {
            "outer wall/inner wall"
        } else {
            "inner wall/outer wall"
        }
        .into(),
        "seam_slope_type" => match raw {
            "nowhere" => "none",
            "contours" => "external",
            "everywhere" => "all",
            _ => return None,
        }
        .into(),
        "seam_slope_conditional" | "seam_slope_entire_loop" | "seam_slope_inner_walls" => {
            if bool_raw(raw)? { "1" } else { "0" }.into()
        }
        "seam_slope_min_length" => num(raw.parse::<f64>().ok().filter(|v| v.is_finite())?),
        "seam_slope_start_height" => {
            let (v, _) = percent_or_mm(raw)?;
            format!("{}%", num(v))
        }
        "seam_slope_steps" => {
            let segment = raw.parse::<f64>().ok().filter(|v| *v > 0.)?;
            num((scarf_length(c) / segment - 1e-9).ceil().max(1.))
        }
        _ => return None,
    })
}

/// Whether an Orca value and a PrusaSlicer value say the same.
pub fn agree(c: &Config, pair: &Pair, orca: &str, prusa: &str) -> bool {
    let Some(projected) = to_prusa(c, pair, orca) else {
        return false;
    };
    let same = |a: &str, b: &str| match (percent_or_mm(a), percent_or_mm(b)) {
        (Some((x, _)), Some((y, _))) => (x - y).abs() < 1e-6,
        _ => a.trim() == b.trim() || bool_raw(a).is_some_and(|x| bool_raw(b) == Some(x)),
    };
    same(&projected, prusa)
        // A step count is the same if PrusaSlicer's segment length rounds back to it.
        || (pair.orca == "seam_slope_steps"
            && to_orca(c, pair, prusa).is_some_and(|s| same(&s, orca)))
}

/// The Orca value Mica shows and Orca's engine uses: the Orca key, else its translated
/// PrusaSlicer counterpart. Any other key: its own text.
pub fn orca_raw(c: &Config, key: &str) -> Option<String> {
    if let Some(own) = c.get(key).map(crate::config::serialize_value) {
        return Some(own);
    }
    let pair = by_orca(key)?;
    to_orca(c, pair, &c.raw(pair.prusa)?)
}

/// The PrusaSlicer value PrusaSlicer's engine uses and PrusaSlicer files get: the translated
/// Orca key when it is set, else the counterpart itself. Any other key: its own text.
pub fn prusa_raw(c: &Config, key: &str) -> Option<String> {
    if let Some(pair) = by_prusa(key)
        && let Some(orca) = c.get(pair.orca).map(crate::config::serialize_value)
        && let Some(v) = to_prusa(c, pair, &orca)
    {
        return Some(v);
    }
    c.raw(key)
}

fn typed(key: &str, raw: Option<String>) -> Option<ConfigValue> {
    parse_value(option_defs().get(key)?.ty, &raw?).ok()
}

/// [`orca_raw`], typed.
pub fn orca_value(c: &Config, key: &str) -> Option<Value> {
    match typed(key, orca_raw(c, key))? {
        ConfigValue::Scalar(v) => Some(v),
        ConfigValue::Vector(v) => v.into_iter().next().flatten(),
    }
}

/// [`prusa_raw`], typed.
pub fn prusa_value(c: &Config, key: &str) -> Option<Value> {
    match typed(key, prusa_raw(c, key))? {
        ConfigValue::Scalar(v) => Some(v),
        ConfigValue::Vector(v) => v.into_iter().next().flatten(),
    }
}

/// The counterparts to write with an edit of `key` to `raw`, so a PrusaSlicer file (or
/// PrusaSlicer itself) reading only the counterpart sees the same value: `(key, raw)` pairs,
/// counterpart first. Editing the scarf length also re-derives the segment length when the
/// step count is the Orca-side value.
pub fn edits_with_counterparts(c: &Config, key: &str, raw: &str) -> Vec<(String, String)> {
    let Some(pair) = by_orca(key) else {
        return vec![(key.into(), raw.into())];
    };
    let mut after = c.clone();
    if after.set_raw(key, raw).is_err() {
        return vec![(key.into(), raw.into())];
    }
    let mut out = Vec::new();
    if let Some(p) = to_prusa(&after, pair, raw) {
        out.push((pair.prusa.to_string(), p));
    }
    if key == "seam_slope_min_length"
        && let Some(steps) = c
            .get("seam_slope_steps")
            .map(crate::config::serialize_value)
        && let Some(p) = to_prusa(&after, by_orca("seam_slope_steps").unwrap(), &steps)
    {
        out.push(("scarf_seam_max_segment_length".into(), p));
    }
    out.push((key.into(), raw.into()));
    out
}

/// Sets the PrusaSlicer counterparts of every Orca key the configuration holds (typed, so the
/// Orca keys stay): what a PrusaSlicer file of this configuration must say.
pub fn project_to_prusa(c: &mut Config) {
    for pair in PAIRS {
        if c.get(pair.orca).is_some()
            && let Some(v) = typed(pair.prusa, prusa_raw(c, pair.prusa))
        {
            c.set(pair.prusa, v);
        }
    }
}

/// After mapping an OrcaSlicer configuration: the PrusaSlicer counterparts follow the Orca
/// keys, so PrusaSlicer rules and PrusaSlicer files see Orca's values. A `full` configuration
/// (a whole Orca profile) gives a missing Orca key Orca's default; a sparse one leaves it out.
pub fn complete_from_orca(c: &mut Config, full: bool) {
    // The step translation reads the scarf length, so it goes last.
    for pair in PAIRS {
        let raw = match c.get(pair.orca) {
            Some(v) => crate::config::serialize_value(v),
            None if full => option_defs()[pair.orca].default.to_string(),
            None => continue,
        };
        if let Some(v) = typed(pair.prusa, to_prusa(c, pair, &raw)) {
            c.set(pair.prusa, v);
        }
    }
}

/// A PrusaSlicer-dialect layer of raw settings (an INI preset, a PrusaSlicer project) that Mica
/// wrote carries both spellings. If they disagree, PrusaSlicer (or a person) edited the
/// counterpart afterwards, so the Orca key goes. `base` supplies the layer height and scarf
/// length the translation needs when the layer does not set them.
pub fn reconcile_prusa_layer(layer: &mut BTreeMap<String, String>, base: &Config) {
    let mut c = base.clone();
    for (k, v) in layer.iter() {
        let _ = c.set_raw(k, v);
    }
    for pair in PAIRS {
        if let (Some(o), Some(p)) = (layer.get(pair.orca), layer.get(pair.prusa))
            && !agree(&c, pair, o, p)
        {
            layer.remove(pair.orca);
        }
    }
}

/// How Mica labels objects (`gcode_label_objects`). The firmware decides the commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelObjects {
    Disabled,
    /// `; printing object …` comments (OctoPrint's Cancel Object plugin).
    Octoprint,
    /// M486 on Marlin, EXCLUDE_OBJECT on Klipper.
    Firmware,
}

pub fn label_objects(c: &Config) -> LabelObjects {
    match c.string("gcode_label_objects") {
        Some("firmware") => LabelObjects::Firmware,
        Some("octoprint") => LabelObjects::Octoprint,
        _ => LabelObjects::Disabled,
    }
}

/// `gcode_label_objects` for OrcaSlicer's two switches (Label objects, Exclude objects).
pub fn label_objects_from_orca(label: bool, exclude: bool) -> &'static str {
    match (label, exclude) {
        (_, true) => "firmware",
        (true, false) => "octoprint",
        (false, false) => "disabled",
    }
}

/// OrcaSlicer's `(gcode_label_objects, exclude_object)` for a `gcode_label_objects` value.
pub fn label_objects_to_orca(value: &str) -> (&'static str, &'static str) {
    match value.trim() {
        "firmware" => ("0", "1"),
        "octoprint" => ("1", "0"),
        _ => ("0", "0"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(h: &str) -> Config {
        let mut c = Config::defaults();
        c.set_raw("layer_height", h).unwrap();
        c
    }

    #[test]
    fn every_pair_is_an_optional_orca_key_with_a_prusa_counterpart() {
        for p in PAIRS {
            assert_eq!(
                crate::config::orca_table(p.orca),
                Some(crate::config::OrcaTable::Optional),
                "{}",
                p.orca
            );
            assert!(crate::config::orca_table(p.prusa).is_none(), "{}", p.prusa);
            assert!(Config::defaults().get(p.prusa).is_some());
            // Within one layer the counterpart is applied before the Orca key (key order).
            assert!(p.prusa < p.orca, "{} sorts after {}", p.prusa, p.orca);
            // Orca's default translates.
            let d = option_defs()[p.orca].default;
            assert!(to_prusa(&layer("0.2"), p, d).is_some(), "{}", p.orca);
        }
    }

    #[test]
    fn values_translate_both_ways() {
        let c = layer("0.2");
        let pair = |k| by_orca(k).unwrap();
        let ws = pair("wall_sequence");
        assert_eq!(to_prusa(&c, ws, "outer wall/inner wall").unwrap(), "1");
        assert_eq!(to_prusa(&c, ws, "inner-outer-inner wall").unwrap(), "0");
        assert_eq!(to_orca(&c, ws, "1").unwrap(), "outer wall/inner wall");
        assert!(agree(&c, ws, "inner-outer-inner wall", "0"));
        assert!(!agree(&c, ws, "inner-outer-inner wall", "1"));
        let t = pair("seam_slope_type");
        assert_eq!(to_prusa(&c, t, "all").unwrap(), "everywhere");
        assert_eq!(to_orca(&c, t, "contours").unwrap(), "external");
        let h = pair("seam_slope_start_height");
        assert_eq!(to_prusa(&c, h, "0.05").unwrap(), "25%");
        assert_eq!(to_prusa(&c, h, "30%").unwrap(), "30%");
        assert_eq!(to_prusa(&c, h, "0.5").unwrap(), "100%");
        assert_eq!(to_orca(&c, h, "40%").unwrap(), "40%");
        let s = pair("seam_slope_steps");
        assert_eq!(to_prusa(&c, s, "10").unwrap(), "2");
        assert_eq!(to_prusa(&c, s, "1000").unwrap(), "0.15");
        assert_eq!(to_orca(&c, s, "1").unwrap(), "20");
        assert_eq!(to_orca(&c, s, "0.7").unwrap(), "29");
        assert!(agree(&c, s, "20", "1"));
        assert!(agree(&c, s, "29", "0.7"));
        assert!(!agree(&c, s, "10", "1"));
    }

    #[test]
    fn the_orca_key_wins_and_the_counterpart_alone_replaces_it() {
        let mut c = Config::defaults();
        assert_eq!(
            orca_raw(&c, "wall_sequence").unwrap(),
            "inner wall/outer wall"
        );
        assert_eq!(orca_raw(&c, "seam_slope_conditional").unwrap(), "1");
        assert_eq!(orca_raw(&c, "seam_slope_steps").unwrap(), "20");
        c.set_raw("wall_sequence", "outer wall/inner wall").unwrap();
        assert_eq!(prusa_raw(&c, "external_perimeters_first").unwrap(), "1");
        assert_eq!(c.raw("external_perimeters_first").unwrap(), "0");
        project_to_prusa(&mut c);
        assert_eq!(c.raw("external_perimeters_first").unwrap(), "1");
        assert!(c.get("wall_sequence").is_some());
        c.set_raw("external_perimeters_first", "0").unwrap();
        assert!(c.get("wall_sequence").is_none());
        assert_eq!(
            orca_raw(&c, "wall_sequence").unwrap(),
            "inner wall/outer wall"
        );
        let edits = edits_with_counterparts(&c, "seam_slope_type", "external");
        assert_eq!(
            edits,
            [
                ("scarf_seam_placement".to_string(), "contours".to_string()),
                ("seam_slope_type".into(), "external".into())
            ]
        );
    }

    #[test]
    fn orca_profiles_get_counterparts_from_orca_defaults() {
        let mut c = Config::defaults();
        c.set_raw("seam_slope_type", "all").unwrap();
        complete_from_orca(&mut c, true);
        assert_eq!(c.raw("scarf_seam_placement").unwrap(), "everywhere");
        // Orca's default (off), not PrusaSlicer's (on).
        assert_eq!(c.raw("scarf_seam_only_on_smooth").unwrap(), "0");
        assert_eq!(orca_raw(&c, "seam_slope_conditional").unwrap(), "0");
        assert_eq!(c.raw("scarf_seam_max_segment_length").unwrap(), "2");
        assert!(c.get("seam_slope_conditional").is_none());
        let mut sparse = Config::new();
        sparse.set_raw("seam_slope_inner_walls", "1").unwrap();
        complete_from_orca(&mut sparse, false);
        assert_eq!(sparse.raw("scarf_seam_on_inner_perimeters").unwrap(), "1");
        assert!(sparse.get("scarf_seam_placement").is_none());
    }

    #[test]
    fn a_prusa_layer_edited_outside_mica_drops_the_stale_orca_key() {
        let base = layer("0.2");
        let mut l: BTreeMap<String, String> = [
            ("external_perimeters_first", "1"),
            ("wall_sequence", "inner wall/outer wall"),
            ("scarf_seam_placement", "everywhere"),
            ("seam_slope_type", "all"),
            ("scarf_seam_start_height", "25%"),
            ("seam_slope_start_height", "0.05"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        reconcile_prusa_layer(&mut l, &base);
        assert!(!l.contains_key("wall_sequence"));
        assert_eq!(l["seam_slope_type"], "all");
        assert_eq!(l["seam_slope_start_height"], "0.05");
    }

    #[test]
    fn object_labels() {
        assert_eq!(label_objects_from_orca(true, true), "firmware");
        assert_eq!(label_objects_from_orca(true, false), "octoprint");
        assert_eq!(label_objects_from_orca(false, false), "disabled");
        for v in ["firmware", "octoprint", "disabled"] {
            let (l, e) = label_objects_to_orca(v);
            assert_eq!(label_objects_from_orca(l == "1", e == "1"), v);
        }
    }
}
