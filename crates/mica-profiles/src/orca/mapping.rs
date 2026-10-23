// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::extrude_loop
// Source: orca@8500fcdcca src/libslic3r/PrintConfig.cpp :: PrintConfigDef::init_fff_params, PrintConfigDef::handle_legacy, PrintConfigDef::handle_legacy_composite
// Source: prusaslicer@b028299c77 src/libslic3r/PrintConfig.cpp :: PrintConfigDef::init_fff_params
// Source: orca@8500fcdcca src/libslic3r/Support/SupportParameters.hpp :: SupportParameters::SupportParameters
// Source: orca@8500fcdcca src/libslic3r/PrintConfig.hpp :: is_auto, is_tree
// Source: orca@8500fcdcca src/libslic3r/Support/TreeSupportCommon.hpp :: TreeSupportMeshGroupSettings::TreeSupportMeshGroupSettings
// Changes: Explicit conservative mapping table, base/unit/enum conversions, lossless raw values,
// Changes: one disposition per input key, strict invalid-value diagnostics instead of substitution.
// Changes: map_sparse maps without Mica defaults (a project's own values, compared to its presets).
// Changes: Merged settings (mica_core::merged): Label objects + Exclude objects become gcode_label_objects; the PrusaSlicer counterparts of Orca's wall sequence and scarf keys follow them.
// Changes: support_type/support_style resolve to support_material_auto and the effective style;
// Changes: organic or (for classic tree styles) classic tree settings and the tree wall count map
// Changes: to Prusa's support_tree_* options; the wall count also selects the support sheath.
// Source: orca@8500fcdcca src/libslic3r/Support/TreeSupport.cpp :: TreeSupport::TreeSupport
// Changes: A mapped Orca profile slices with OrcaSlicer rules unless it records mica_slicing_rules; z_hop_types typed.
// Changes: Sparse project and object values go through handle_legacy (legacy.rs) as in Orca; vertical shell
// Changes: modes map both ways; top_one_wall_type keeps Topmost; no curr_bed_type reads the smooth PEI plate.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use mica_core::config::{
    Config, ConfigValue, OptType, Value as TypedValue, option_defs, parse_value,
};
use serde_json::Value;

use super::{Dialect, RawConfig, schema};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    Mapped,
    Converted,
    ExtrasOnly,
    Rejected,
}
#[derive(Clone, Debug)]
pub struct MappingEntry {
    pub key: String,
    pub canonical: Option<String>,
    pub disposition: Disposition,
    pub reason: String,
}
#[derive(Clone, Debug, Default)]
pub struct MappingReport {
    pub entries: Vec<MappingEntry>,
    /// What the user should know about values that were read in a way of Mica's own (a
    /// Snapmaker Orca flow-variant list whose filaments would read different values).
    pub notes: Vec<String>,
}
impl MappingReport {
    pub fn count(&self, disposition: Disposition) -> usize {
        self.entries
            .iter()
            .filter(|e| e.disposition == disposition)
            .count()
    }
}
#[derive(Clone, Debug)]
pub struct MappingResult {
    pub config: Config,
    /// All input JSON values, including invalid/unknown values and known-key collisions.
    pub orca: RawConfig,
    /// Every source key in Orca option-text form, including names already canonical in Config.
    pub extras: BTreeMap<String, String>,
    pub report: MappingReport,
}

/// The executable, source-reviewed table. Each entry is `(canonical key, conversion)`.
pub fn mapping_table() -> &'static BTreeMap<&'static str, (&'static str, &'static str)> {
    static TABLE: OnceLock<BTreeMap<&'static str, (&'static str, &'static str)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        include_str!("mapping.tsv")
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
            .map(|l| {
                let mut f = l.split('\t');
                (f.next().unwrap(), (f.next().unwrap(), f.next().unwrap()))
            })
            .collect()
    })
}

/// Canonical keys a conversion sets besides its table target (for listing a project's changes).
pub fn also_sets(key: &str) -> &'static [&'static str] {
    match key {
        "support_type" => &["support_material_style"],
        "tree_support_wall_count" => &["support_material_with_sheath"],
        _ => &[],
    }
}

fn one(value: &Value) -> Result<&str, String> {
    if let Some(s) = value.as_str() {
        return Ok(s);
    }
    let a = value.as_array().ok_or("expected string or vector")?;
    let first = a
        .first()
        .and_then(Value::as_str)
        .ok_or("empty vector has no scalar value")?;
    if a.iter().any(|v| v.as_str() != Some(first)) {
        return Err("multiple unequal values cannot become a scalar".into());
    }
    Ok(first)
}
fn process_scalar(raw: &RawConfig, key: &str, value: &Value) -> Result<String, String> {
    if let Ok(s) = one(value) {
        return Ok(s.into());
    }
    let flows = raw
        .get("process_flow_support")
        .and_then(Value::as_array)
        .ok_or("unequal vector needs a selected process flow variant")?;
    let selected = raw
        .get("nozzle_volume_type")
        .ok_or("missing selected nozzle flow variant")?;
    let selected = one(selected)?;
    let index = flows
        .iter()
        .position(|v| v.as_str() == Some(selected))
        .ok_or("selected nozzle flow is absent from process_flow_support")?;
    value
        .as_array()
        .and_then(|a| a.get(index))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("{key}: missing selected flow segment {index}"))
}
fn elements(value: &Value) -> Result<Vec<String>, String> {
    match value {
        Value::String(s) => Ok(vec![s.clone()]),
        Value::Array(a) => a
            .iter()
            .map(|s| {
                s.as_str()
                    .map(str::to_string)
                    .ok_or("expected string element".into())
            })
            .collect(),
        _ => Err("expected string or array".into()),
    }
}

fn absolute(raw: &RawConfig, dialect: Dialect, key: &str, depth: usize) -> Result<f64, String> {
    if depth > 8 {
        return Err("cyclic percentage base".into());
    }
    let value = raw
        .get(key)
        .ok_or_else(|| format!("missing percentage base {key}"))?;
    let scalar = process_scalar(raw, key, value)?;
    let s = scalar.as_str();
    let n = schema::finite_number(s.trim_end_matches('%'))?;
    if !s.ends_with('%') {
        return Ok(n);
    }
    let def = schema::schema(dialect)
        .get(key)
        .ok_or_else(|| format!("unknown percentage base for {key}"))?;
    let base = def["ratio"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("unknown percentage base for {key}"))?;
    Ok(n * absolute(raw, dialect, base, depth + 1)? / 100.0)
}

fn enum_value(key: &str, s: &str) -> Option<&'static str> {
    match key {
        "enable_arc_fitting" => match s {
            "1" => Some("emit_center"),
            "0" => Some("disabled"),
            _ => None,
        },
        // As both slicers read their old booleans: 1 = all / enabled, 0 = moderate / partial.
        "ensure_vertical_shell_thickness" => match s {
            "ensure_all" => Some("enabled"),
            "ensure_moderate" | "ensure_critical_only" => Some("partial"),
            "none" => Some("disabled"),
            _ => None,
        },
        "seam_position" => match s {
            "nearest" => Some("nearest"),
            "aligned" => Some("aligned"),
            "back" => Some("rear"),
            "random" => Some("random"),
            _ => None,
        },
        "gcode_flavor" => match s {
            "klipper" => Some("klipper"),
            "marlin" => Some("marlin"),
            "marlin2" => Some("marlin2"),
            "reprapfirmware" => Some("reprapfirmware"),
            "reprap" => Some("reprap"),
            "repetier" => Some("repetier"),
            "teacup" => Some("teacup"),
            "makerware" => Some("makerware"),
            "sailfish" => Some("sailfish"),
            "smoothie" => Some("smoothie"),
            "mach3" => Some("mach3"),
            "machinekit" => Some("machinekit"),
            "no-extrusion" => Some("no-extrusion"),
            _ => None,
        },
        "sparse_infill_pattern" | "top_surface_pattern" | "bottom_surface_pattern" => match s {
            "rectilinear" | "zig-zag" => Some("rectilinear"),
            "monotonic" => Some("monotonic"),
            "monotonicline" => Some("monotoniclines"),
            "alignedrectilinear" => Some("alignedrectilinear"),
            "grid" => Some("grid"),
            "triangles" => Some("triangles"),
            "tri-hexagon" => Some("stars"),
            "cubic" => Some("cubic"),
            "crosshatch" => Some("crosshatch"),
            "line" => Some("line"),
            "concentric" => Some("concentric"),
            "honeycomb" => Some("honeycomb"),
            "3dhoneycomb" => Some("3dhoneycomb"),
            "gyroid" => Some("gyroid"),
            "hilbertcurve" => Some("hilbertcurve"),
            "archimedeanchords" => Some("archimedeanchords"),
            "octagramspiral" => Some("octagramspiral"),
            "adaptivecubic" => Some("adaptivecubic"),
            "supportcubic" => Some("supportcubic"),
            "lightning" => Some("lightning"),
            _ => None,
        },
        "wall_generator" => match s {
            "classic" => Some("classic"),
            "arachne" => Some("arachne"),
            _ => None,
        },
        "brim_type" => match s {
            "no_brim" => Some("no_brim"),
            "auto_brim" => Some("auto_brim"),
            "brim_ears" => Some("brim_ears"),
            "painted" => Some("painted"),
            "outer_only" => Some("outer_only"),
            "inner_only" => Some("inner_only"),
            "outer_and_inner" => Some("outer_and_inner"),
            _ => None,
        },
        "draft_shield" => match s {
            "disabled" => Some("disabled"),
            "enabled" => Some("enabled"),
            "limited" => Some("limited"),
            _ => None,
        },
        "slicing_mode" => match s {
            "regular" => Some("regular"),
            "even_odd" => Some("even_odd"),
            "close_holes" => Some("close_holes"),
            _ => None,
        },
        "fuzzy_skin" => match s {
            "none" | "disabled_fuzzy" => Some("none"),
            "external" => Some("external"),
            "all" => Some("all"),
            _ => None,
        },
        "printer_technology" => match s {
            "FFF" => Some("FFF"),
            _ => None,
        },
        "ironing_type" => match s {
            "top" => Some("top"),
            "topmost" => Some("topmost"),
            "all" => Some("solid"),
            _ => None,
        },
        _ => None,
    }
}
/// Orca `support_type` (`normal|tree` + `(auto)|(manual)`) and `support_style` as Mica's
/// `support_material_auto` and effective `support_material_style` (Orca `SupportParameters`):
/// manual types support only enforcers, tree types default to organic, Orca's classic tree styles
/// become Mica's tree, normal types default to grid. `None` for an unknown support type.
pub fn support_mode(support_type: &str, style: &str) -> Option<(bool, &'static str)> {
    let (tree, auto) = match support_type {
        "normal(auto)" => (false, true),
        "tree(auto)" => (true, true),
        "normal(manual)" => (false, false),
        "tree(manual)" => (true, false),
        _ => return None,
    };
    let style = match (tree, style) {
        (true, "tree_slim" | "tree_strong" | "tree_hybrid") => "tree",
        (true, _) => "organic",
        (false, "snug") => "snug",
        (false, _) => "grid",
    };
    Some((auto, style))
}

fn bed_prefix(raw: &RawConfig) -> Option<&'static str> {
    match raw.get("curr_bed_type")?.as_str()? {
        "Cool Plate" => Some("cool_plate"),
        "Engineering Plate" => Some("eng_plate"),
        "High Temp Plate" | "Smooth PEI Plate" => Some("hot_plate"),
        "Textured PEI Plate" => Some("textured_plate"),
        "Textured Cool Plate" => Some("textured_cool_plate"),
        "Supertack Plate" | "SuperTack Plate" => Some("supertack_plate"),
        _ => None,
    }
}

/// Convert without silently dropping input. Every key gets exactly one report entry. Values absent
/// from the source use Mica defaults; unsupported modes remain visible in the report and raw map.
/// A full Orca profile slices with OrcaSlicer's rules unless it records Mica's
/// `mica_slicing_rules` itself.
pub fn map_config(raw: &RawConfig, dialect: Dialect) -> MappingResult {
    let (raw, notes) = super::flow_variant::resolved(raw, dialect);
    let raw = &raw;
    let mut result = map_into(raw, dialect, Config::defaults());
    result.report.notes = notes;
    // A whole Orca profile: the PrusaSlicer counterparts of the merged settings take the
    // profile's (or Orca's default) values, so PrusaSlicer rules and files see Orca's choice.
    mica_core::merged::complete_from_orca(&mut result.config, true);
    explicit_rules(raw)
        .unwrap_or(mica_core::rules::Rules::OrcaSlicer)
        .set(&mut result.config);
    super::gcode_config::remember(&mut result.config, raw, dialect);
    result
}

/// The `mica_slicing_rules` an Orca file records (Mica's sidecar or a Mica-written preset).
pub fn explicit_rules(raw: &RawConfig) -> Option<mica_core::rules::Rules> {
    raw.get(mica_core::rules::KEY)
        .and_then(Value::as_str)
        .and_then(mica_core::rules::Rules::parse)
}

/// Map a configuration without filling in Mica's defaults: the result holds only what `raw`
/// sets (a project's embedded settings, compared against the presets they are based on).
pub fn map_sparse(raw: &RawConfig, dialect: Dialect) -> MappingResult {
    let (raw, notes) = super::flow_variant::resolved(raw, dialect);
    // OrcaSlicer reads every project value through handle_legacy.
    let raw = super::legacy::parsed(&raw, dialect);
    let mut result = map_into(&raw, dialect, Config::new());
    result.report.notes = notes;
    mica_core::merged::complete_from_orca(&mut result.config, false);
    super::gcode_config::remember(&mut result.config, &raw, dialect);
    result
}

/// Map sparse 3MF object/volume settings through the same dialect table as profiles.
pub fn map_metadata(raw: &BTreeMap<String, String>, dialect: Dialect) -> MappingResult {
    let raw = raw
        .iter()
        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
        .collect();
    let (raw, notes) = super::flow_variant::resolved(&raw, dialect);
    let raw = super::legacy::parsed(&raw, dialect);
    let mut result = map_into(&raw, dialect, Config::new());
    result.report.notes = notes;
    mica_core::merged::complete_from_orca(&mut result.config, false);
    super::gcode_config::remember(&mut result.config, &raw, dialect);
    result
}

fn map_into(raw: &RawConfig, dialect: Dialect, mut config: Config) -> MappingResult {
    let mut report = MappingReport::default();
    let mut extras = BTreeMap::new();
    // Filament overrides must run after printer retraction, regardless of lexical key order.
    let mut keys: Vec<_> = raw.keys().collect();
    keys.sort_by_key(|k| {
        mapping_table()
            .get(k.as_str())
            .is_some_and(|(_, op)| *op == "override")
    });
    for key in keys {
        let value = &raw[key];
        let mut entry = MappingEntry {
            key: key.clone(),
            canonical: None,
            disposition: Disposition::ExtrasOnly,
            reason: "no implemented canonical equivalent; original value retained".into(),
        };
        let ty = schema::schema(dialect)
            .get(key)
            .and_then(|d| d["type"].as_str())
            .unwrap_or("");
        let text = schema::option_text(value, ty).unwrap_or_else(|_| value.to_string());
        extras.insert(key.clone(), text.clone());
        // Known-name collisions stay out of Config's extras on purpose (templates would read the raw
        // string instead of the typed value); the sidecar keeps their original text.
        if !option_defs().contains_key(key.as_str()) {
            config
                .set_raw(key, &text)
                .expect("unknown key is always accepted as an extra");
        }
        // Orca's exporter accepts zero as automatic small-perimeter speed despite the UI min=1.
        let normalized = if key == "small_perimeter_speed" {
            let mut def = schema::schema(dialect)[key].clone();
            def["min"] = Value::from(0);
            schema::normalized(value, &def)
        } else if ty == "String" {
            value
                .as_str()
                .map(|_| value.clone())
                .ok_or("expected string".into())
        } else if let Some(def) = schema::schema(dialect).get(key) {
            schema::normalized(value, def)
        } else {
            schema::option_text(value, ty).map(|_| value.clone())
        };
        let result = normalized.and_then(|v| {
            // Mica's own options mean the same in both dialects. Without this an Orca project's
            // object and part values (`mica_texture_paint_slots`, textures, brick layers) were
            // dropped as "no canonical equivalent" when it was opened, and texture paint lost
            // its slots.
            if key == "mica_gcode_identity"
                || (key.starts_with("mica_") && option_defs().contains_key(key.as_str()))
            {
                let text = v.as_str().map_or_else(|| text.clone(), str::to_string);
                config.set_raw(key, &text).map_err(|e| e.to_string())?;
                entry.canonical = Some(key.clone());
                return Ok(Some(false));
            }
            if !option_defs().contains_key(key.as_str())
                && let Ok(mut typed) = schema::typed(&v, ty)
            {
                // Orca resolves scalar percentage options against its own ratio_over key.
                if ty == "FloatOrPercent"
                    && schema::schema(dialect)[key]["ratio"]
                        .as_str()
                        .is_some_and(|s| !s.is_empty())
                {
                    typed = ConfigValue::Scalar(TypedValue::Float(absolute(raw, dialect, key, 0)?));
                }
                config.set(key, typed);
            }
            let Some(&(target, op)) = mapping_table().get(key.as_str()) else {
                return Ok(None);
            };
            entry.canonical = Some(target.into());
            convert(&mut config, raw, dialect, key, target, op, &v, ty)
        });
        match result {
            Ok(Some(converted)) => {
                entry.disposition = if converted {
                    Disposition::Converted
                } else {
                    Disposition::Mapped
                };
                entry.reason = if converted {
                    "converted units, vocabulary, or value shape"
                } else {
                    "identical meaning and units"
                }
                .into();
            }
            Ok(None) => {}
            Err(reason) => {
                entry.disposition = Disposition::Rejected;
                entry.reason = reason;
            }
        }
        report.entries.push(entry);
    }
    report.entries.sort_by(|a, b| a.key.cmp(&b.key));
    MappingResult {
        config,
        orca: raw.clone(),
        extras,
        report,
    }
}

#[allow(clippy::too_many_arguments)]
fn convert(
    config: &mut Config,
    raw: &RawConfig,
    dialect: Dialect,
    key: &str,
    target: &str,
    op: &str,
    value: &Value,
    source_ty: &str,
) -> Result<Option<bool>, String> {
    // Engine-specific Orca settings keep their typed source schema instead of inventing a Prusa alias.
    if op == "engine" {
        return Ok(Some(false));
    }
    let def = &option_defs()[target];
    if key == "nozzle_diameter"
        && (elements(value)?.is_empty()
            || elements(value)?
                .iter()
                .any(|s| schema::finite_number(s).is_ok_and(|v| v <= 0.0)))
    {
        return Err("nozzle diameters must be a nonempty vector of positive numbers".into());
    }
    let changed_shape = !def.ty.is_vector() && value.is_array();
    let mut value = value.clone();
    if !def.ty.is_vector() && value.is_array() {
        value = Value::String(process_scalar(raw, key, &value)?);
    }
    let mut converted = op != "identity" || changed_shape;
    match op {
        "top_wall" => {
            // BambuStudio's top_one_wall_type keeps PrusaSlicer's Topmost surface only.
            let topmost = raw
                .get("top_one_wall_type")
                .and_then(|v| one(v).ok())
                .is_some_and(|s| s == "topmost");
            value = Value::String(
                match (one(&value)?, topmost) {
                    ("1", true) => "topmost",
                    ("1", false) => "top",
                    _ => "none",
                }
                .into(),
            );
        }
        "enum" => {
            if key == "ironing_type" && one(&value)? == "no ironing" {
                config.set_raw("ironing", "0").map_err(|e| e.to_string())?;
                return Ok(Some(true));
            }
            let Some(mapped) = enum_value(key, one(&value)?) else {
                return Ok(None);
            };
            value = Value::String(mapped.into());
            if key == "ironing_type" {
                config.set_raw("ironing", "1").map_err(|e| e.to_string())?;
            }
        }
        "sequence" => {
            value = Value::String(
                match one(&value)? {
                    "by layer" => "0",
                    "by object" => "1",
                    _ => return Ok(None),
                }
                .into(),
            )
        }
        "walls" => {
            value = Value::String(
                match one(&value)? {
                    "inner wall/outer wall" => "0",
                    "outer wall/inner wall" => "1",
                    _ => return Ok(None),
                }
                .into(),
            )
        }
        // One Label objects setting: Orca's Label objects and Exclude objects switches.
        "labels" | "exclude" => {
            let on = |v: Option<&Value>| {
                v.and_then(|v| one(v).ok())
                    .is_some_and(|s| matches!(s, "1" | "true"))
            };
            let this = matches!(one(&value)?, "1" | "true");
            let (label, exclude) = if op == "labels" {
                (this, on(raw.get("exclude_object")))
            } else {
                (on(raw.get("gcode_label_objects")), this)
            };
            value = Value::String(mica_core::merged::label_objects_from_orca(label, exclude).into())
        }
        "width" | "absolute" => {
            if one(&value)?.ends_with('%') {
                let number = absolute(raw, dialect, key, 0)?;
                if op == "width" && number <= 0.0 {
                    return Err("percentage width requires a positive nozzle diameter".into());
                }
                value = Value::String(number.to_string());
            }
        }
        "support_type" | "support_style" => {
            let text = |v: Option<&Value>| v.and_then(|v| one(v).ok()).map(str::to_string);
            let (kind, style) = if key == "support_type" {
                (one(&value)?.to_string(), text(raw.get("support_style")))
            } else {
                (
                    text(raw.get("support_type")).unwrap_or_default(),
                    Some(one(&value)?.to_string()),
                )
            };
            let style = style.unwrap_or_else(|| "default".into());
            let Some((auto, effective)) = support_mode(&kind, &style) else {
                // A sparse style without its type: only an explicit Mica style is unambiguous.
                return Ok(match (key, style.as_str()) {
                    ("support_style", "grid" | "snug") => {
                        config.set_raw(target, &style).map_err(|e| e.to_string())?;
                        Some(true)
                    }
                    ("support_style", "organic") => {
                        config
                            .set_raw(target, "organic")
                            .map_err(|e| e.to_string())?;
                        Some(true)
                    }
                    ("support_style", "tree_slim" | "tree_strong" | "tree_hybrid") => {
                        config.set_raw(target, "tree").map_err(|e| e.to_string())?;
                        Some(true)
                    }
                    ("support_type", _) => return Err(format!("unknown support type {kind}")),
                    _ => None,
                });
            };
            config
                .set_raw("support_material_style", effective)
                .map_err(|e| e.to_string())?;
            if key == "support_type" {
                config
                    .set_raw("support_material_auto", if auto { "1" } else { "0" })
                    .map_err(|e| e.to_string())?;
            }
            return Ok(Some(true));
        }
        "tree_classic" | "tree_organic" => {
            // Orca keeps separate organic and classic (tree_slim/strong/hybrid) branch settings;
            // Mica's tree and organic styles share support_tree_*. Apply the set the effective
            // style uses; a sparse key without support_type keeps the organic default.
            let text = |v: Option<&Value>| v.and_then(|v| one(v).ok()).map(str::to_string);
            let classic = text(raw.get("support_type")).and_then(|kind| {
                let style = text(raw.get("support_style")).unwrap_or_else(|| "default".into());
                support_mode(&kind, &style).map(|(_, s)| s == "tree")
            });
            if (op == "tree_classic") != classic.unwrap_or(false) {
                return Ok(None);
            }
            converted = changed_shape;
        }
        "tree_walls" => {
            let count = one(&value)?;
            // Orca SupportParameters: with_sheath = tree_support_wall_count > 0, also for
            // normal supports (Prusa's support_material_with_sheath defaults on).
            config
                .set_raw(
                    "support_material_with_sheath",
                    if count == "0" { "0" } else { "1" },
                )
                .map_err(|e| e.to_string())?;
            // Double-wall area: 0 -> islands above a 5 mm circle, 1 -> never, 2+ -> everywhere
            // (Orca's 0.1 scaled² threshold, i.e. any printable island).
            value = Value::String(
                match count {
                    "0" => "5",
                    "1" => "0",
                    _ => "0.001",
                }
                .into(),
            )
        }
        "percent" => value = Value::String(format!("{}%", one(&value)?.trim_end_matches('%'))),
        "shrink" => {
            value = Value::Array(
                elements(&value)?
                    .iter()
                    .map(|s| {
                        schema::finite_number(s.trim_end_matches('%'))
                            .map(|v| Value::String((100.0 - v).to_string()))
                    })
                    .collect::<Result<_, _>>()?,
            )
        }
        "bed" => {
            // Without a plate type (a printer Orca has none for), Mica writes the smooth PEI
            // plate's temperatures.
            let prefix = if raw.contains_key("curr_bed_type") {
                bed_prefix(raw)
            } else {
                Some("hot_plate")
            };
            let Some(prefix) = prefix else {
                return Err("select curr_bed_type to map bed temperatures".into());
            };
            if !key.starts_with(&format!("{prefix}_temp")) {
                return Ok(None);
            }
        }
        "override" => {
            let source = elements(&value)?;
            let Some(ConfigValue::Vector(previous)) = config.get(target) else {
                return Err("filament override target is not a vector".into());
            };
            let mut output = previous.clone();
            if output.is_empty() {
                return Err("cannot apply filament override to an empty printer vector".into());
            }
            if let Some(first) = output.first().cloned() {
                output.resize(source.len(), first);
            }
            for (i, s) in source.iter().enumerate() {
                if s != "nil" {
                    let parsed = parse_value(def.ty, s)?;
                    if let ConfigValue::Vector(v) = parsed {
                        output[i] = v[0].clone();
                    }
                }
            }
            config.set(target, ConfigValue::Vector(output));
            return Ok(Some(true));
        }
        _ => {}
    }
    // Older Snapmaker process settings are vectors for flow variants. Equal entries can collapse;
    // unequal entries require a selected flow variant and must not silently pick the first.
    if !def.ty.is_vector() && value.is_array() {
        value = Value::String(process_scalar(raw, key, &value)?);
        converted = true;
    }
    if def.ty.is_vector() && value.is_string() {
        value = Value::Array(vec![value]);
        converted = true;
    }
    let target_ty = match def.ty {
        OptType::String => "String",
        OptType::Strings => "Strings",
        _ => source_ty,
    };
    if matches!(source_ty, "Percent" | "Percents")
        && matches!(def.ty, OptType::FloatOrPercent | OptType::FloatsOrPercents)
    {
        let add = |s: &str| Value::String(format!("{}%", s.trim_end_matches('%')));
        value = match &value {
            Value::String(s) => add(s),
            Value::Array(a) => Value::Array(a.iter().map(|v| add(v.as_str().unwrap())).collect()),
            _ => value,
        };
    }
    let text = schema::serialize_normalized(&value, target_ty)?;
    let parsed = parse_value(def.ty, &text)?;
    // Mica's general integer parser rounds; imports reject fractional values instead.
    if matches!(def.ty, OptType::Int | OptType::Ints) {
        for s in elements(&value)? {
            if schema::finite_number(&s)?.fract() != 0.0 {
                return Err("fractional value cannot map to an integer".into());
            }
        }
    }
    if !def.nullable && matches!(&parsed,ConfigValue::Vector(v) if v.iter().any(Option::is_none)) {
        return Err("nil is not allowed in canonical option".into());
    }
    // Validate floating point results, including overflow in percentage calculations.
    let finite = |v: &TypedValue| match v {
        TypedValue::Float(v)
        | TypedValue::Percent(v)
        | TypedValue::FloatOrPercent { value: v, .. } => v.is_finite(),
        TypedValue::Point(x, y) => x.is_finite() && y.is_finite(),
        _ => true,
    };
    if !match &parsed {
        ConfigValue::Scalar(v) => finite(v),
        ConfigValue::Vector(v) => v.iter().flatten().all(finite),
    } {
        return Err("non-finite conversion result".into());
    }
    config.set(target, parsed);
    Ok(Some(converted))
}

#[cfg(test)]
mod region_tests {
    use super::*;
    #[test]
    fn metadata_mapping_stays_sparse_and_preserves_alias_conversions() {
        let raw = BTreeMap::from([
            ("wall_loops".into(), "4".into()),
            ("sparse_infill_density".into(), "100%".into()),
            ("outer_wall_speed".into(), "50".into()),
            ("enable_support".into(), "0".into()),
        ]);
        let result = map_metadata(&raw, Dialect::Orca242);
        assert_eq!(result.config.int("perimeters"), Some(4));
        assert_eq!(result.config.float("fill_density"), Some(100.));
        assert_eq!(result.config.float("external_perimeter_speed"), Some(50.));
        assert_eq!(result.config.bool("support_material"), Some(false));
        assert!(result.config.get("layer_height").is_none());
        assert!(result.config.get("nozzle_diameter").is_none());
        assert_eq!(result.report.count(Disposition::Rejected), 0);
    }

    /// What Mica wrote for a PrusaSlicer printer before it wrote Orca's values (overnight
    /// stability campaign) reads as meant, OrcaSlicer's own values and legacy too; an old
    /// percentage speed or XY distance is dropped, as OrcaSlicer's handle_legacy drops it.
    #[test]
    fn project_values_read_like_orca_including_older_mica_spellings() {
        let raw: RawConfig = [
            ("enable_arc_fitting", "emit_center"),
            ("ensure_vertical_shell_thickness", "partial"),
            ("only_one_wall_top", "topmost"),
            ("hot_plate_temp", "67"),
            ("support_interface_speed", "80%"),
            ("support_object_xy_distance", "60%"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), Value::String(v.into())))
        .collect();
        let r = map_sparse(&raw, Dialect::Orca242);
        assert_eq!(r.report.count(Disposition::Rejected), 0, "{:?}", r.report);
        assert_eq!(r.config.string("arc_fitting"), Some("emit_center"));
        assert_eq!(
            r.config.string("ensure_vertical_shell_thickness"),
            Some("partial")
        );
        assert_eq!(r.config.string("top_one_perimeter_type"), Some("topmost"));
        assert_eq!(r.config.raw("bed_temperature").as_deref(), Some("67"));
        assert!(r.config.get("support_material_interface_speed").is_none());
        assert!(r.config.get("support_material_xy_spacing").is_none());
        for (orca, prusa) in [
            ("1", "enabled"),
            ("0", "partial"),
            ("ensure_all", "enabled"),
            ("ensure_critical_only", "partial"),
            ("none", "disabled"),
        ] {
            let raw = BTreeMap::from([("ensure_vertical_shell_thickness".into(), orca.into())]);
            let r = map_metadata(&raw, Dialect::Orca242);
            assert_eq!(
                r.config.string("ensure_vertical_shell_thickness"),
                Some(prusa),
                "{orca}"
            );
        }
        for (only, kind, prusa) in [
            ("1", None, "top"),
            ("0", None, "none"),
            ("1", Some("topmost"), "topmost"),
            ("0", Some("all top"), "top"),
            ("1", Some("none"), "top"),
        ] {
            let mut raw = BTreeMap::from([("only_one_wall_top".to_string(), only.to_string())]);
            if let Some(kind) = kind {
                raw.insert("top_one_wall_type".into(), kind.into());
            }
            let r = map_metadata(&raw, Dialect::Orca242);
            assert_eq!(
                r.config.string("top_one_perimeter_type"),
                Some(prusa),
                "{only} {kind:?}"
            );
        }
    }

    #[test]
    fn support_type_selects_enforcers_only_and_the_effective_style() {
        for (kind, style, auto, effective) in [
            ("normal(auto)", "default", true, "grid"),
            ("normal(auto)", "snug", true, "snug"),
            ("normal(manual)", "organic", false, "grid"),
            ("tree(auto)", "default", true, "organic"),
            ("tree(manual)", "default", false, "organic"),
            ("tree(manual)", "snug", false, "organic"),
            ("tree(manual)", "tree_hybrid", false, "tree"),
            ("tree(auto)", "tree_slim", true, "tree"),
        ] {
            assert_eq!(support_mode(kind, style), Some((auto, effective)));
            let raw: RawConfig = [
                ("support_type".to_string(), Value::String(kind.into())),
                ("support_style".to_string(), Value::String(style.into())),
            ]
            .into_iter()
            .collect();
            let result = map_config(&raw, Dialect::Orca242);
            assert_eq!(
                result.config.bool("support_material_auto"),
                Some(auto),
                "{kind}"
            );
            assert_eq!(
                result.config.string("support_material_style"),
                Some(effective),
                "{kind} {style}"
            );
            assert_eq!(
                result.report.count(Disposition::Converted),
                2,
                "{:?}",
                result.report
            );
        }
        assert_eq!(support_mode("hybrid", "default"), None);
        // Sparse object settings: a type alone uses Orca's default style; an Orca style alone
        // is only applied when it names one Mica style unambiguously.
        let only_type = BTreeMap::from([("support_type".into(), "tree(manual)".into())]);
        let r = map_metadata(&only_type, Dialect::Orca242);
        assert_eq!(r.config.bool("support_material_auto"), Some(false));
        assert_eq!(r.config.string("support_material_style"), Some("organic"));
        let only_style = BTreeMap::from([("support_style".into(), "default".into())]);
        let r = map_metadata(&only_style, Dialect::Orca242);
        assert!(r.config.get("support_material_style").is_none());
        // Classic Orca tree styles use their own branch settings; organic uses *_organic.
        let branches = |style: &str| {
            let raw: RawConfig = [
                ("support_type", "tree(auto)"),
                ("support_style", style),
                ("tree_support_branch_angle", "45"),
                ("tree_support_branch_angle_organic", "40"),
                ("tree_support_branch_diameter", "5"),
                ("tree_support_branch_diameter_organic", "2"),
                ("tree_support_branch_distance", "5"),
                ("tree_support_branch_distance_organic", "1"),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), Value::String(v.into())))
            .collect();
            let c = map_config(&raw, Dialect::Orca242).config;
            (
                c.float("support_tree_angle"),
                c.float("support_tree_branch_diameter"),
                c.float("support_tree_branch_distance"),
            )
        };
        assert_eq!(branches("tree_hybrid"), (Some(45.), Some(5.), Some(5.)));
        assert_eq!(branches("tree_slim"), (Some(45.), Some(5.), Some(5.)));
        assert_eq!(branches("default"), (Some(40.), Some(2.), Some(1.)));
        assert_eq!(branches("organic"), (Some(40.), Some(2.), Some(1.)));
        let sparse = BTreeMap::from([("tree_support_branch_angle".into(), "50".into())]);
        assert!(
            map_metadata(&sparse, Dialect::Orca242)
                .config
                .get("support_tree_angle")
                .is_none()
        );
        let walls = |n: &str| {
            let raw = BTreeMap::from([("tree_support_wall_count".into(), n.into())]);
            map_metadata(&raw, Dialect::Orca242)
                .config
                .float("support_tree_branch_diameter_double_wall")
        };
        assert_eq!(walls("0"), Some(5.));
        assert_eq!(walls("1"), Some(0.));
        assert_eq!(walls("2"), Some(0.001));
        let sheath = |n: &str| {
            let raw = BTreeMap::from([("tree_support_wall_count".into(), n.into())]);
            map_metadata(&raw, Dialect::Orca242)
                .config
                .bool("support_material_with_sheath")
        };
        assert_eq!(sheath("0"), Some(false));
        assert_eq!(sheath("1"), Some(true));
        assert_eq!(sheath("2"), Some(true));
    }
}
