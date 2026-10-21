// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/PrintConfig.cpp :: PrintConfigDef::init_fff_params
// Source: prusaslicer@b028299c77 src/libslic3r/PrintConfig.cpp :: PrintConfigDef::init_fff_params
// Changes: Project export names and conservative reversible value conversions; original raw settings remain in the Mica marker.
// Changes: mica_* keys never enter an Orca project_settings.config; they go to the sidecar.
// Changes: filament_settings_id converts between Prusa's quoted list and Orca's JSON array.
// Changes: Merged settings (crate::merged): wall order and scarf values translate both ways, the Orca key winning over its counterpart; Label objects becomes Orca's Label objects + Exclude objects.
// Changes: Arc fitting, one wall on top (with BambuStudio's top_one_wall_type for Topmost) and vertical shell modes in each dialect's vocabulary; bed temperatures name their plate type.

use super::{
    Flavour, Project, RawConfig, Settings, ThreeMfError, invalid,
    json::{self, Json},
};
use crate::config::{
    OptType, escape_string, escape_strings, option_defs, unescape_string, unescape_strings,
};

pub(super) fn source(p: &Project) -> Flavour {
    if let Some(c) = &p.config {
        return if c.path.ends_with("project_settings.config") {
            Flavour::Orca
        } else {
            Flavour::Prusa
        };
    }
    if p.config
        .as_ref()
        .is_some_and(|c| c.path.ends_with("project_settings.config"))
        || p.objects
            .iter()
            .any(|o| o.parts.iter().any(|p| p.metadata.contains_key("subtype")))
    {
        Flavour::Orca
    } else {
        Flavour::Prusa
    }
}
/// The dialect a project's settings are written in: Orca for `project_settings.config` (or
/// Orca part metadata), else PrusaSlicer. Object, part and range settings of a `Project` must
/// use this dialect; see [`translate`].
pub fn source_flavour(p: &Project) -> Flavour {
    source(p)
}
/// Translate sparse settings between the dialects: canonical (PrusaSlicer-named) settings an
/// editor keeps become Orca names and values when the project is written with an Orca
/// configuration. Options without an equivalent keep their spelling.
pub fn translate(s: &Settings, from: Flavour, to: Flavour) -> Settings {
    patch(s, from, to)
}
fn rules() -> impl Iterator<Item = (&'static str, &'static str, &'static str)> {
    include_str!("write_mapping.tsv")
        .lines()
        .filter(|s| !s.starts_with('#') && !s.is_empty())
        .map(|s| {
            let mut a = s.split('\t');
            (a.next().unwrap(), a.next().unwrap(), a.next().unwrap())
        })
}
/// Translate a sparse patch. Options without an equivalent retain their source spelling;
/// the marker also retains the exact original patch and dialect.
pub(super) fn patch(s: &Settings, from: Flavour, to: Flavour) -> Settings {
    if from == to {
        return s.clone();
    }
    // The merged settings' translations read neighbouring values (layer height, scarf length).
    let context = {
        let mut c = crate::config::Config::new();
        for (k, v) in s {
            let _ = c.set_raw(k, v);
        }
        c
    };
    let on = |k: &str| s.get(k).is_some_and(|v| matches!(v.trim(), "1" | "true"));
    let mut mapped: Settings = s
        .iter()
        .filter(|(k, _)| {
            !matches!(
                k.as_str(),
                "support_type"
                    | "support_style"
                    | "support_material_auto"
                    | "support_material_style"
            ) && !(to == Flavour::Prusa && k.as_str() == "top_one_wall_type")
        })
        .filter_map(|(k, v)| {
            let rule = rules().find(|(o, p, op)| match to {
                Flavour::Prusa => *o == k,
                Flavour::Orca => {
                    *p == k && *op != "override" && (*op != "bed" || o.starts_with("hot_plate"))
                }
            });
            let Some((o, p, op)) = rule else {
                return Some((k.clone(), v.clone()));
            };
            let key = if to == Flavour::Orca { o } else { p };
            if op == "merged" {
                let pair = crate::merged::by_orca(o).expect("write_mapping.tsv: merged pair");
                return match to {
                    // The Orca key itself wins over its translated counterpart.
                    Flavour::Orca if s.contains_key(o) => return None,
                    Flavour::Orca => crate::merged::to_orca(&context, pair, v),
                    Flavour::Prusa => crate::merged::to_prusa(&context, pair, v),
                }
                .or_else(|| Some(v.clone()))
                .map(|v| (key.to_string(), v));
            }
            if let Some(value) = vocabulary(o, to, v.trim(), s) {
                return Some((key.into(), value.into()));
            }
            let value = match (op, to, v.as_str()) {
                ("labels", Flavour::Orca, v) => crate::merged::label_objects_to_orca(v).0,
                ("labels", Flavour::Prusa, v) => crate::merged::label_objects_from_orca(
                    matches!(v.trim(), "1" | "true"),
                    on("exclude_object"),
                ),
                ("exclude", Flavour::Prusa, v) => crate::merged::label_objects_from_orca(
                    on("gcode_label_objects"),
                    matches!(v.trim(), "1" | "true"),
                ),
                ("sequence", Flavour::Orca, "0") => "by layer",
                ("sequence", Flavour::Orca, "1") => "by object",
                ("sequence", Flavour::Prusa, "by layer") => "0",
                ("sequence", Flavour::Prusa, "by object") => "1",
                ("enum", Flavour::Orca, "monotoniclines") => "monotonicline",
                ("enum", Flavour::Prusa, "monotonicline") => "monotoniclines",
                ("enum", Flavour::Orca, "stars") => "tri-hexagon",
                ("enum", Flavour::Prusa, "tri-hexagon") => "stars",
                ("enum", Flavour::Orca, "marlin2") => "marlin",
                ("enum", Flavour::Prusa, "marlin") => "marlin2",
                _ => v,
            };
            // PrusaSlicer stores shrinkage compensation (0 % = none), OrcaSlicer the remaining size
            // (100 % = none): the inverse of mica-profiles' Orca import rule.
            if op == "shrink" {
                let converted = value
                    .split(',')
                    .map(|s| {
                        let n = s.trim().trim_end_matches('%').parse::<f64>().unwrap_or(0.);
                        format!("{}%", 100. - n)
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                return Some((key.into(), converted));
            }
            Some((key.into(), value.into()))
        })
        .collect();
    if to == Flavour::Orca {
        // PrusaSlicer's Topmost surface only has no Orca switch value: BambuStudio's
        // `top_one_wall_type` (which Orca's handle_legacy still reads as the switch on) keeps
        // it, so the project reads back as it was saved.
        if let Some(v) = s.get("top_one_perimeter_type") {
            let kind = match v.trim() {
                "topmost" => "topmost",
                "top" => "all top",
                _ => "none",
            };
            mapped.insert("top_one_wall_type".into(), kind.into());
        }
        // Orca keeps one bed temperature per plate type and reads the selected plate's: a
        // printer without Orca plate types writes Orca's smooth PEI plate (as PrusaSlicer's
        // sheet), and says so.
        if mapped.keys().any(|k| k.starts_with("hot_plate_temp"))
            && !s.contains_key("curr_bed_type")
        {
            mapped.insert("curr_bed_type".into(), DEFAULT_BED.into());
        }
    }
    match to {
        // Orca's Exclude objects goes with Label objects (one setting in Mica).
        Flavour::Orca if s.contains_key("gcode_label_objects") => {
            let (_, exclude) = crate::merged::label_objects_to_orca(&s["gcode_label_objects"]);
            mapped.insert("exclude_object".into(), exclude.into());
        }
        // A PrusaSlicer project keeps the merged Orca keys next to their counterparts (PrusaSlicer
        // ignores them), so Mica reads back what PrusaSlicer cannot say (Inner/Outer/Inner).
        Flavour::Prusa => {
            for pair in crate::merged::PAIRS {
                if let Some(v) = s.get(pair.orca) {
                    mapped.insert(pair.orca.into(), v.clone());
                }
            }
        }
        _ => {}
    }
    match to {
        Flavour::Orca => {
            if s.contains_key("support_material_auto") || s.contains_key("support_material_style") {
                let auto = s.get("support_material_auto").is_none_or(|v| v != "0");
                let style = s
                    .get("support_material_style")
                    .map_or("grid", String::as_str);
                let (family, native_style) = match style {
                    "tree" => ("tree", "tree_slim"),
                    "organic" => ("tree", "default"),
                    "snug" => ("normal", "snug"),
                    _ => ("normal", "default"),
                };
                mapped.insert(
                    "support_type".into(),
                    format!("{family}({})", if auto { "auto" } else { "manual" }),
                );
                mapped.insert("support_style".into(), native_style.into());
            }
        }
        Flavour::Prusa => {
            if let Some(kind) = s.get("support_type") {
                let tree = kind.starts_with("tree(");
                let style = s.get("support_style").map_or("default", String::as_str);
                mapped.insert(
                    "support_material_auto".into(),
                    if kind.ends_with("(manual)") { "0" } else { "1" }.into(),
                );
                mapped.insert(
                    "support_material_style".into(),
                    match (tree, style) {
                        (true, "tree_slim" | "tree_strong" | "tree_hybrid") => "tree",
                        (true, _) => "organic",
                        (false, "snug") => "snug",
                        _ => "grid",
                    }
                    .into(),
                );
            } else if let Some(style) = s.get("support_style") {
                let effective = match style.as_str() {
                    "snug" => Some("snug"),
                    "tree_slim" | "tree_strong" | "tree_hybrid" => Some("tree"),
                    "organic" => Some("organic"),
                    _ => None,
                };
                if let Some(style) = effective {
                    mapped.insert("support_material_style".into(), style.into());
                }
            }
        }
    }
    mapped
}
/// The plate type Mica writes bed temperatures for when the settings name none.
const DEFAULT_BED: &str = "High Temp Plate";

/// The Orca key prefix of a plate type's bed temperatures (Orca's `BedType` options).
fn bed_prefix(bed: &str) -> Option<&'static str> {
    Some(match bed {
        "Cool Plate" => "cool_plate",
        "Engineering Plate" => "eng_plate",
        "High Temp Plate" | "Smooth PEI Plate" => "hot_plate",
        "Textured PEI Plate" => "textured_plate",
        "Textured Cool Plate" => "textured_cool_plate",
        "Supertack Plate" | "SuperTack Plate" => "supertack_plate",
        _ => return None,
    })
}

/// Values whose vocabulary differs between the dialects (Orca `orca`'s value for PrusaSlicer's
/// `value`, or the other way round): PrusaSlicer's arc fitting mode is Orca's switch, Orca's
/// one-wall-on-top switch is PrusaSlicer's surface choice, and the vertical shell modes
/// correspond the way both slicers read their old booleans (`1` all / enabled, `0`
/// moderate / partial; Orca's Critical only, which Mica never writes, reads as partial).
fn vocabulary(orca: &str, to: Flavour, value: &str, s: &Settings) -> Option<&'static str> {
    Some(match (orca, to, value) {
        ("enable_arc_fitting", Flavour::Orca, "disabled") => "0",
        ("enable_arc_fitting", Flavour::Orca, "emit_center" | "emit_radius") => "1",
        ("enable_arc_fitting", Flavour::Prusa, "0") => "disabled",
        ("enable_arc_fitting", Flavour::Prusa, "1") => "emit_center",
        ("only_one_wall_top", Flavour::Orca, "none") => "0",
        ("only_one_wall_top", Flavour::Orca, "top" | "topmost") => "1",
        ("only_one_wall_top", Flavour::Prusa, "0") => "none",
        ("only_one_wall_top", Flavour::Prusa, "1") => {
            match s.get("top_one_wall_type").map(|v| v.trim()) {
                Some("topmost") => "topmost",
                _ => "top",
            }
        }
        ("ensure_vertical_shell_thickness", Flavour::Orca, "enabled") => "ensure_all",
        ("ensure_vertical_shell_thickness", Flavour::Orca, "partial") => "ensure_moderate",
        ("ensure_vertical_shell_thickness", Flavour::Orca, "disabled") => "none",
        ("ensure_vertical_shell_thickness", Flavour::Prusa, "ensure_all" | "1") => "enabled",
        (
            "ensure_vertical_shell_thickness",
            Flavour::Prusa,
            "ensure_moderate" | "ensure_critical_only" | "0",
        ) => "partial",
        ("ensure_vertical_shell_thickness", Flavour::Prusa, "none") => "disabled",
        _ => return None,
    })
}

fn ini(text: &str) -> Settings {
    text.lines()
        .filter_map(|l| {
            let l = l.trim().trim_start_matches(';').trim();
            l.split_once('=')
                .map(|(k, v)| (k.trim().into(), v.trim().into()))
        })
        .collect()
}
fn canonical_key(key: &str) -> &str {
    rules()
        .find(|(o, _, _)| *o == key)
        .map_or(key, |(_, p, _)| p)
}
/// The option type of `key`. Preset bookkeeping outside the option table is typed too:
/// `filament_settings_id` is a list of names in both dialects (one per filament), and was
/// written to Orca projects as Prusa-style text (`"a";"b"`), which neither OrcaSlicer nor
/// Mica's CLI reads as a list (GUI monkey test, seed 1008).
fn option_type(key: &str) -> Option<OptType> {
    match key {
        "filament_settings_id" => Some(OptType::Strings),
        _ => option_defs().get(canonical_key(key)).map(|d| d.ty),
    }
}
fn from_json(j: &Json) -> Result<Settings, ThreeMfError> {
    j.map()?
        .iter()
        .map(|(k, v)| {
            let ty = option_type(k);
            let text = match v {
                Json::Array(a) => {
                    let a = a
                        .iter()
                        .map(|v| v.text().map(str::to_owned))
                        .collect::<Result<Vec<_>, _>>()?;
                    if matches!(ty, Some(OptType::Strings)) {
                        escape_strings(&a)
                    } else {
                        a.join(",")
                    }
                }
                Json::String(s) if matches!(ty, Some(OptType::String)) => escape_string(s),
                Json::String(s) | Json::Atom(s) => s.clone(),
                _ => v.encode(),
            };
            Ok((k.clone(), text))
        })
        .collect()
}
fn to_json(s: &Settings) -> Json {
    Json::object(s.iter().map(|(k, v)| {
        let ty = option_type(k);
        let value = match ty {
            _ if matches!(k.as_str(), "wipe_tower_x" | "wipe_tower_y") => {
                Json::Array(v.split(',').map(|s| Json::from(s.trim())).collect())
            }
            Some(OptType::Strings) => Json::Array(
                unescape_strings(v)
                    .unwrap_or_else(|_| vec![v.clone()])
                    .into_iter()
                    .map(Json::from)
                    .collect(),
            ),
            Some(t) if t.is_vector() => {
                Json::Array(v.split(',').map(|s| Json::from(s.trim())).collect())
            }
            Some(OptType::String) => Json::from(unescape_string(v).unwrap_or_else(|_| v.clone())),
            _ => Json::from(v.clone()),
        };
        (k.clone(), value)
    }))
}
pub(super) fn config(p: &Project, to: Flavour) -> Result<String, ThreeMfError> {
    let from = source(p);
    // Preserve native JSON/INI verbatim, including unknown settings and escaped templates.
    if let Some(c) = &p.config
        && from == to
        && (to == Flavour::Prusa || p.wipe_towers.is_empty() || orca_towers(c)? == p.wipe_towers)
    {
        if to == Flavour::Orca {
            json::parse(&c.text)?.map()?;
        }
        return Ok(c.text.clone());
    }
    let raw = match &p.config {
        Some(c) if from == Flavour::Orca => from_json(&json::parse(&c.text)?)?,
        Some(c) => ini(&c.text),
        None => Settings::new(),
    };
    let mapped = patch(&raw, from, to);
    if to == Flavour::Prusa {
        let mut text = String::from("; generated by Mica Slicer\n");
        for (k, v) in mapped {
            // Orca per-plate position vectors become the first bed's scalar position.
            let v = if matches!(
                k.as_str(),
                "wipe_tower_x" | "wipe_tower_y" | "wipe_tower_rotation_angle"
            ) {
                v.split(',').next().unwrap_or("").into()
            } else {
                v
            };
            text.push_str(&format!("; {k} = {v}\n"));
        }
        return Ok(text);
    }
    let mut json = if from == to
        && let Some(c) = &p.config
    {
        json::parse(&c.text)?
    } else {
        to_json(&mapped)
    };
    let Json::Object(ref mut map) = json else {
        return Err(invalid("project settings must be an object"));
    };
    if !p.wipe_towers.is_empty() {
        for (axis, key) in ["wipe_tower_x", "wipe_tower_y"].into_iter().enumerate() {
            let old = map.get(key).and_then(|v| v.array().ok());
            let count = p
                .plates
                .len()
                .max(p.wipe_towers.keys().last().copied().unwrap_or(0) as usize + 1);
            let values = (0..count)
                .map(|i| {
                    p.wipe_towers
                        .get(&(i as u32))
                        .map(|v| Json::from(v[axis].to_string()))
                        .or_else(|| old.and_then(|a| a.get(i)).cloned())
                        .unwrap_or_else(|| Json::from("0"))
                })
                .collect();
            map.insert(key.into(), Json::Array(values));
        }
        // Orca stores rotation globally; differing per-bed rotations stay in mica.json.
        if let Some(v) = p.wipe_towers.get(&0) {
            map.insert(
                "wipe_tower_rotation_angle".into(),
                Json::from(v[2].to_string()),
            );
        }
    }
    Ok(json.encode())
}

pub(super) fn orca_towers(
    c: &RawConfig,
) -> Result<std::collections::BTreeMap<u32, [f64; 3]>, ThreeMfError> {
    let mut out = std::collections::BTreeMap::new();
    if !c.path.ends_with("project_settings.config") {
        return Ok(out);
    }
    let j = json::parse(&c.text)?;
    let map = j.map()?;
    if let (Some(xs), Some(ys)) = (map.get("wipe_tower_x"), map.get("wipe_tower_y")) {
        let rotation = map.get("wipe_tower_rotation_angle");
        let xs = xs.array().unwrap_or_else(|_| std::slice::from_ref(xs));
        let ys = ys.array().unwrap_or_else(|_| std::slice::from_ref(ys));
        for (i, (x, y)) in xs.iter().zip(ys).enumerate() {
            let p = [
                x.num::<f64>()?,
                y.num::<f64>()?,
                rotation
                    .map(|v| v.array().ok().and_then(|a| a.get(i)).unwrap_or(v))
                    .map(|v| v.num())
                    .transpose()?
                    .unwrap_or(0.),
            ];
            if p.iter().any(|v| !v.is_finite()) {
                return Err(invalid("non-finite tower position"));
            }
            out.insert(i as u32, p);
        }
    }
    Ok(out)
}

impl Project {
    /// Overlay canonical (Prusa-named) project settings while retaining unknown options.
    /// Values use `Config::raw` syntax, including string escaping and typed vectors.
    pub fn update_settings(&mut self, values: &Settings) -> Result<(), ThreeMfError> {
        let flavour = source(self);
        let mut values = values.clone();
        // OrcaSlicer files carry only Orca options: Mica's own go to the sidecar.
        if flavour == Flavour::Orca {
            let mica: Vec<String> = values
                .keys()
                .filter(|k| k.starts_with("mica_"))
                .cloned()
                .collect();
            for key in mica {
                let value = values.remove(&key).expect("listed key");
                self.mica_settings.insert(key, value);
            }
        }
        if flavour == Flavour::Orca
            && (values.contains_key("support_material_auto")
                || values.contains_key("support_material_style"))
            && let Some(c) = &self.config
        {
            let existing = patch(
                &from_json(&json::parse(&c.text)?)?,
                Flavour::Orca,
                Flavour::Prusa,
            );
            for key in ["support_material_auto", "support_material_style"] {
                if let Some(value) = existing.get(key) {
                    values.entry(key.into()).or_insert_with(|| value.clone());
                }
            }
        }
        let mut mapped = patch(&values, Flavour::Prusa, flavour);
        let text = if flavour == Flavour::Orca {
            let mut j = match &self.config {
                Some(c) => json::parse(&c.text)?,
                None => Json::object(Vec::<(String, Json)>::new()),
            };
            let Json::Object(ref mut object) = j else {
                return Err(invalid("project settings must be an object"));
            };
            // Bed temperatures go to the plate type the project already selects.
            if !values.contains_key("curr_bed_type")
                && let Some(bed) = object.get("curr_bed_type").and_then(|v| v.text().ok())
                && let Some(prefix) = bed_prefix(bed)
            {
                mapped.remove("curr_bed_type");
                for suffix in ["", "_initial_layer"] {
                    if let Some(v) = mapped.remove(&format!("hot_plate_temp{suffix}")) {
                        mapped.insert(format!("{prefix}_temp{suffix}"), v);
                    }
                }
            }
            // Keep every list as long as the project's: per-filament (and per-extruder) options
            // must all have one entry per slot, or OrcaSlicer rejects the project ("filament_is_support's
            // count 4 not equal to filament_colour's size 1"). Values resolved for fewer slots are
            // broadcast/padded with their last entry; extra entries are dropped.
            let lengths: std::collections::BTreeMap<String, usize> = object
                .iter()
                .filter_map(|(k, v)| v.array().ok().map(|a| (k.clone(), a.len())))
                .collect();
            let filaments = object
                .get("filament_colour")
                .and_then(|v| v.array().ok())
                .map_or(0, <[Json]>::len);
            for (k, mut v) in to_json(&mapped).map()?.clone() {
                // Mica's typed vectors arrive as Prusa-style text ("0,0,0,0", "\"a\";\"b\""); Orca stores
                // vectors as JSON arrays.
                let vector =
                    lengths.contains_key(&k) || (k.starts_with("filament_") && filaments > 0);
                if vector && let Json::String(text) = &v {
                    v = Json::Array(split_vector(text).into_iter().map(Json::from).collect());
                }
                let want = lengths
                    .get(&k)
                    .copied()
                    .or((k.starts_with("filament_") && filaments > 0).then_some(filaments));
                if let (Some(want), Json::Array(values)) = (want, &mut v)
                    && want > 0
                    && values.len() != want
                    && let Some(last) = values.last().cloned()
                {
                    values.resize(want, last);
                }
                object.insert(k, v);
            }
            j.encode()
        } else {
            let mut all = self
                .config
                .as_ref()
                .map_or_else(Settings::new, |c| ini(&c.text));
            all.extend(mapped);
            all.into_iter()
                .map(|(k, v)| format!("; {k} = {v}\n"))
                .collect()
        };
        self.config = Some(RawConfig {
            path: if flavour == Flavour::Orca {
                "Metadata/project_settings.config"
            } else {
                "Metadata/Slic3r_PE.config"
            }
            .into(),
            text,
        });
        Ok(())
    }
}

/// Split a Prusa-style serialised vector: `"a";"b"` (strings) or `0,0,1` (numbers and bools).
fn split_vector(text: &str) -> Vec<String> {
    if text.starts_with('"') {
        unescape_strings(text).unwrap_or_else(|_| vec![text.to_string()])
    } else {
        text.split(',').map(|s| s.trim().to_string()).collect()
    }
}

#[cfg(test)]
mod orca_vector_tests {
    use super::*;

    #[test]
    fn prusa_vectors_split_for_orca_json() {
        assert_eq!(split_vector("0,0,1,0"), ["0", "0", "1", "0"]);
        assert_eq!(split_vector("\"a b\";\"c\""), ["a b", "c"]);
    }

    #[test]
    fn shrinkage_converts_between_dialects() {
        let mut s = Settings::new();
        s.insert(
            "filament_shrinkage_compensation_xy".into(),
            "0%,1.5%".into(),
        );
        let orca = patch(&s, Flavour::Prusa, Flavour::Orca);
        assert_eq!(orca["filament_shrink"], "100%,98.5%");
    }

    /// PrusaSlicer values become values OrcaSlicer reads (overnight stability campaign: a GUI
    /// save of a CORE One project kept `enable_arc_fitting = emit_center`, `only_one_wall_top =
    /// none`, `ensure_vertical_shell_thickness = enabled` and bed temperatures without a plate
    /// type), and read back as they were.
    #[test]
    fn prusa_vocabulary_becomes_orca_values_and_back() {
        let s: Settings = [
            ("arc_fitting", "emit_center"),
            ("ensure_vertical_shell_thickness", "partial"),
            ("top_one_perimeter_type", "topmost"),
            ("bed_temperature", "60"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let orca = patch(&s, Flavour::Prusa, Flavour::Orca);
        assert_eq!(orca["enable_arc_fitting"], "1");
        assert_eq!(orca["ensure_vertical_shell_thickness"], "ensure_moderate");
        assert_eq!(orca["only_one_wall_top"], "1");
        assert_eq!(orca["top_one_wall_type"], "topmost");
        assert_eq!(orca["hot_plate_temp"], "60");
        assert_eq!(orca["curr_bed_type"], "High Temp Plate");
        let back = patch(&orca, Flavour::Orca, Flavour::Prusa);
        for (k, v) in &s {
            assert_eq!(back.get(k), Some(v), "{k}");
        }
        assert!(!back.contains_key("top_one_wall_type"));
        for (prusa, orca_value) in [
            ("disabled", "none"),
            ("enabled", "ensure_all"),
            ("partial", "ensure_moderate"),
        ] {
            let s = Settings::from([("ensure_vertical_shell_thickness".into(), prusa.into())]);
            let o = patch(&s, Flavour::Prusa, Flavour::Orca);
            assert_eq!(o["ensure_vertical_shell_thickness"], orca_value);
            assert_eq!(patch(&o, Flavour::Orca, Flavour::Prusa), s);
        }
    }

    /// An edited bed temperature goes to the plate type the Orca project selects.
    #[test]
    fn bed_temperatures_follow_the_projects_plate_type() {
        let mut p = Project {
            config: Some(RawConfig {
                path: "Metadata/project_settings.config".into(),
                text: r#"{"curr_bed_type":"Textured PEI Plate","textured_plate_temp":["55"]}"#
                    .into(),
            }),
            ..Default::default()
        };
        p.update_settings(&Settings::from([("bed_temperature".into(), "65".into())]))
            .unwrap();
        let text = &p.config.unwrap().text;
        assert!(text.contains(r#""textured_plate_temp":["65"]"#), "{text}");
        assert!(
            text.contains(r#""curr_bed_type":"Textured PEI Plate""#),
            "{text}"
        );
        assert!(!text.contains("hot_plate_temp"), "{text}");
    }

    /// GUI monkey test (seed 1008): an XL project saved for Orca kept its filament names as one
    /// Prusa-style string, so `mica slice` found no project filaments and fell back to the
    /// CORE One default ("not compatible with Original Prusa XL").
    #[test]
    fn filament_settings_id_is_a_list_in_both_dialects() {
        let mut s = Settings::new();
        s.insert(
            "filament_settings_id".into(),
            "\"Filatech PLA @XL\";\"Kimya PEBA-S @XL\"".into(),
        );
        let json = to_json(&s);
        let names = json.map().unwrap()["filament_settings_id"]
            .array()
            .unwrap()
            .iter()
            .map(|v| v.text().unwrap().to_string())
            .collect::<Vec<_>>();
        assert_eq!(names, ["Filatech PLA @XL", "Kimya PEBA-S @XL"]);
        assert_eq!(
            from_json(&json).unwrap()["filament_settings_id"],
            s["filament_settings_id"]
        );
    }
}
