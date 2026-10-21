// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! `cargo xtask gen-option-text [orca-dir] [prusaslicer-dir]`: regenerates the GUI's setting
//! labels, tooltips, ranges and page layout.
//!
//! Inputs, from the pinned `orca` and `prusaslicer` checkouts (default `upstream/<id>`):
//! - Orca labels, tooltips, units, ranges and Simple/Advanced mode from
//!   `src/libslic3r/PrintConfig.cpp`, keyed by Orca option name;
//! - the Orca page/group layout of the Process, Filament and Printer tabs from
//!   `src/slic3r/GUI/Tab.cpp`;
//! - PrusaSlicer's `src/libslic3r/PrintConfig.cpp` for enum vocabularies (Mica stores
//!   PrusaSlicer values) and as a fallback for options Orca does not have;
//! - `crates/mica-profiles/src/orca/mapping.tsv` to translate Orca names to Mica's canonical
//!   (PrusaSlicer) names, and `crates/mica-core/src/config/prusa_options.tsv` for the set
//!   of canonical options and their scopes.
//!
//! Outputs: `crates/mica-gui/src/settings/option_text.tsv` and `pages.tsv` (with the
//! rules-specific groups of `RULE_GROUPS`), then docs/setting-names.md (`setting_names`).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::process::ExitCode;

const TEXT_OUTPUT: &str = "crates/mica-gui/src/settings/option_text.tsv";
const PAGES_OUTPUT: &str = "crates/mica-gui/src/settings/pages.tsv";

#[derive(Default, Clone, Debug)]
struct Def {
    label: Option<String>,
    full_label: Option<String>,
    category: Option<String>,
    tooltip: Option<String>,
    sidetext: Option<String>,
    min: Option<String>,
    max: Option<String>,
    mode: Option<String>,
    /// (value, label) pairs.
    enums: Vec<(String, String)>,
    /// `def_<key>->enum_def->clone()` or `= def_<key>->enum_values`.
    enum_from: Option<String>,
}

pub fn run(root: &Path, orca_dir: Option<&str>, prusa_dir: Option<&str>) -> ExitCode {
    let orca = orca_dir.map_or_else(|| root.join("upstream/orca"), |d| Path::new(d).into());
    let prusa = prusa_dir.map_or_else(
        || root.join("upstream/prusaslicer"),
        |d| Path::new(d).into(),
    );
    let read = |p: &Path| fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()));
    let inputs = [
        read(&orca.join("src/libslic3r/PrintConfig.cpp")),
        read(&orca.join("src/slic3r/GUI/Tab.cpp")),
        read(&prusa.join("src/libslic3r/PrintConfig.cpp")),
        read(&root.join("crates/mica-profiles/src/orca/mapping.tsv")),
        read(&root.join("crates/mica-core/src/config/prusa_options.tsv")),
    ];
    if inputs.iter().any(Result::is_err) {
        for e in inputs.iter().filter_map(|r| r.as_ref().err()) {
            eprintln!("{e}");
        }
        eprintln!("(fetch upstream sources with scripts/fetch-upstream.sh)");
        return ExitCode::FAILURE;
    }
    let [orca_cpp, orca_tab, prusa_cpp, mapping, mut options] = inputs.map(Result::unwrap);
    options.push_str(include_str!(
        "../../crates/mica-core/src/config/orca_options.tsv"
    ));
    options.push_str(include_str!(
        "../../crates/mica-core/src/config/orca_options_optional.tsv"
    ));

    let orca_defs = parse_defs(&orca_cpp);
    let prusa_defs = parse_defs(&prusa_cpp);
    // Orca → canonical, and canonical → the first Orca key that maps to it.
    let mut orca_to_canonical = BTreeMap::new();
    let mut canonical_to_orca: BTreeMap<String, (String, String)> = BTreeMap::new();
    for line in mapping.lines().filter(|l| !l.starts_with('#')) {
        let mut f = line.split('\t');
        if let (Some(o), Some(p)) = (f.next(), f.next()) {
            let conversion = f.next().unwrap_or("identity").to_string();
            orca_to_canonical.insert(o.to_string(), p.to_string());
            // An Orca filament override (`filament_retract_when_changing_layer` over the
            // printer's `retract_when_changing_layer`) is the Orca name of Mica's filament
            // override (`filament_retract_layer_change`), not of the printer option.
            let canonical = if conversion == "override" {
                format!("filament_{p}")
            } else {
                p.to_string()
            };
            canonical_to_orca
                .entry(canonical)
                .or_insert_with(|| (o.to_string(), conversion));
        }
    }
    // Canonical options with their scope (print / filament / printer).
    let mut scopes: BTreeMap<String, String> = BTreeMap::new();
    for line in options
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() >= 4 && matches!(f[3], "print" | "filament" | "printer") {
            scopes.insert(f[0].to_string(), f[3].to_string());
        }
    }

    // Text per canonical key.
    let mut text = String::new();
    text.push_str("# Setting text for Mica's GUI: key, label, full label, category, unit, min, max, mode, tooltip, enum values (value=Label|…), source, PrusaSlicer label, OrcaSlicer label.\n");
    text.push_str("# Generated by `cargo xtask gen-option-text` from orca@8500fcdcca and prusaslicer@b028299c77 src/libslic3r/PrintConfig.cpp. Do not edit.\n");
    let esc = |s: &str| {
        s.replace('\\', "\\\\")
            .replace('\t', "\\t")
            .replace('\n', "\\n")
    };
    let mut missing = Vec::new();
    for key in scopes.keys() {
        if is_bookkeeping(key) {
            continue;
        }
        let orca_key = canonical_to_orca.get(key).map(|(k, _)| k);
        let orca_def = orca_key.and_then(|k| orca_defs.get(k)).or_else(|| {
            // PrintConfigDef::init_fff_params: Orca declares its filament overrides in a loop
            // over `filament_extruder_override_keys` with the printer option's text.
            orca_key
                .and_then(|k| k.strip_prefix("filament_"))
                .and_then(|b| orca_defs.get(b))
        });
        let prusa_def = prusa_defs.get(key);
        // Values are PrusaSlicer's: Orca's explanation, unit and range only fit when the value
        // is the same quantity (identity, or a filament override of one) or a translated enum.
        // Otherwise keep Orca's name.
        let same_value = canonical_to_orca
            .get(key)
            .is_none_or(|(_, c)| matches!(c.as_str(), "identity" | "enum" | "override"));
        let (def, source) = match (orca_def, prusa_def) {
            (Some(o), Some(p)) if o.label.is_some() && !same_value => (
                Def {
                    label: o.label.clone(),
                    full_label: o.full_label.clone().or(p.full_label.clone()),
                    ..p.clone()
                },
                format!("orca-label:{}", orca_key.unwrap()),
            ),
            (Some(o), _) if o.label.is_some() => (o.clone(), format!("orca:{}", orca_key.unwrap())),
            (_, Some(p)) => (p.clone(), "prusa".to_string()),
            (Some(o), None) => (o.clone(), format!("orca:{}", orca_key.unwrap())),
            (None, None) => {
                missing.push(key.clone());
                (Def::default(), "none".to_string())
            }
        };
        // Filament overrides of printer settings take the base option's text, as PrusaSlicer does.
        let base = key
            .strip_prefix("filament_")
            .and_then(|b| prusa_defs.get(b))
            .filter(|_| def.label.is_none() && prusa_def.is_none_or(|p| p.label.is_none()));
        let def = match base {
            Some(b) => Def {
                label: b.label.clone(),
                full_label: b.full_label.clone(),
                tooltip: b.tooltip.clone(),
                sidetext: b.sidetext.clone(),
                min: b.min.clone(),
                max: b.max.clone(),
                mode: Some("comAdvanced".into()),
                category: Some("Filament overrides".into()),
                ..Def::default()
            },
            None => def,
        };
        // Orca's filament overrides: Advanced, except the four it shows in Simple mode
        // (PrintConfigDef::init_fff_params, "BBS: shown specific filament retract config").
        let def = match canonical_to_orca.get(key) {
            Some((o, c)) if c == "override" => Def {
                mode: Some(
                    if matches!(
                        o.as_str(),
                        "filament_retraction_length"
                            | "filament_z_hop"
                            | "filament_long_retractions_when_cut"
                            | "filament_retraction_distances_when_cut"
                    ) {
                        "comSimple"
                    } else {
                        "comAdvanced"
                    }
                    .into(),
                ),
                category: Some("Filament overrides".into()),
                ..def
            },
            _ => def,
        };
        let label = def
            .label
            .clone()
            .or_else(|| def.full_label.clone())
            .or_else(|| prusa_def.and_then(|p| p.label.clone().or(p.full_label.clone())))
            .unwrap_or_else(|| synthesize_label(key));
        let full_label = def.full_label.clone().unwrap_or_default();
        // Mica offers PrusaSlicer's modes for these; Orca's tooltip describes other ones.
        let prusa_tooltip = matches!(key.as_str(), "ensure_vertical_shell_thickness");
        let tooltip = prusa_def
            .filter(|_| prusa_tooltip)
            .and_then(|p| p.tooltip.clone())
            .or_else(|| def.tooltip.clone())
            .or_else(|| prusa_def.and_then(|p| p.tooltip.clone()))
            .unwrap_or_default();
        let sidetext = def
            .sidetext
            .clone()
            .or_else(|| prusa_def.and_then(|p| p.sidetext.clone()))
            .unwrap_or_else(|| {
                if key.starts_with("machine_max_acceleration") {
                    "mm/s²".into()
                } else if key.starts_with("machine_max_") {
                    "mm/s".into()
                } else {
                    String::new()
                }
            });
        let min = def
            .min
            .clone()
            .or_else(|| prusa_def.and_then(|p| p.min.clone()))
            .unwrap_or_default();
        let max = def
            .max
            .clone()
            .or_else(|| prusa_def.and_then(|p| p.max.clone()))
            .unwrap_or_default();
        let mode = match def.mode.as_deref() {
            Some("comSimple") | None => "simple",
            _ => "advanced",
        };
        // Enum vocabulary is PrusaSlicer's: Mica stores PrusaSlicer values.
        let enums = prusa_def
            .map(|p| resolve_enums(p, &prusa_defs))
            .or_else(|| orca_def.map(|o| resolve_enums(o, &orca_defs)))
            .unwrap_or_default();
        // Fill patterns: offer exactly what Mica's fill module implements, including Orca-only patterns
        // (e.g. the U1's crosshatch), and nothing it doesn't (Prusa's "line"/"zigzag").
        let enums = if key.ends_with("fill_pattern") {
            let supported = supported_fill_patterns();
            let mut all = enums;
            for (v, l) in orca_def
                .map(|o| resolve_enums(o, &orca_defs))
                .unwrap_or_default()
            {
                let v = match v.as_str() {
                    "tri-hexagon" => "stars".to_string(),
                    "monotonicline" => "monotoniclines".to_string(),
                    _ => v,
                };
                if !all.iter().any(|(a, _)| *a == v) {
                    all.push((v, l));
                }
            }
            all.retain(|(v, _)| supported.iter().any(|s| s == v));
            all
        } else if key == "gcode_flavor" {
            // The G-code writer implements Marlin 2 (Prusa Buddy) and Klipper only; the export
            // refuses every other flavor, so don't offer them.
            let mut all = enums;
            all.retain(|(v, _)| v == "marlin2" || v == "klipper");
            all
        } else {
            enums
        };
        let enums: Vec<String> = enums
            .iter()
            .map(|(v, l)| format!("{v}={}", l.replace(['|', '='], " ")))
            .collect();
        let category = def
            .category
            .clone()
            .or_else(|| prusa_def.and_then(|p| p.category.clone()))
            .unwrap_or_default();
        // The upstream names, for docs/setting-names.md.
        let upstream_label = |d: Option<&Def>| {
            d.and_then(|d| d.label.clone().or(d.full_label.clone()))
                .unwrap_or_default()
        };
        text.push_str(&format!(
            "{key}\t{}\t{}\t{}\t{}\t{min}\t{max}\t{mode}\t{}\t{}\t{source}\t{}\t{}\n",
            esc(&label),
            esc(&full_label),
            esc(&category),
            esc(&sidetext),
            esc(&tooltip),
            enums.join("|"),
            esc(&upstream_label(prusa_def)),
            esc(&upstream_label(orca_def)),
        ));
    }
    if !missing.is_empty() {
        eprintln!("note: no upstream definition for {missing:?}; labels synthesized");
    }

    // Page layout: Orca's tabs, with Orca keys translated to canonical names.
    let mut pages = String::new();
    pages.push_str("# Setting pages for Mica's GUI: tab, page, group, line label, key, source. Order matters.\n");
    pages.push_str("# Generated by `cargo xtask gen-option-text` from orca@8500fcdcca src/slic3r/GUI/Tab.cpp (TabPrint::build, TabFilament::build, TabPrinter::build_fff, build_kinematics_page, build_extruder_pages). Do not edit.\n");
    let mut placed: BTreeSet<String> = BTreeSet::new();
    let mut rows: Vec<Row> = Vec::new();
    for (tab, functions) in [
        ("print", vec!["void TabPrint::build()"]),
        ("filament", vec!["void TabFilament::build()"]),
        (
            "printer",
            vec![
                "void TabPrinter::build_fff()",
                "PageShp TabPrinter::build_kinematics_page()",
                "void TabPrinter::build_unregular_pages(",
            ],
        ),
    ] {
        let mut lines = Vec::new();
        for f in functions {
            if let Some(body) = function_body(&orca_tab, f) {
                lines.extend(parse_layout(body));
            }
        }
        for (page, group, line_label, orca_key) in lines {
            let Some(key) = orca_to_canonical.get(&orca_key) else {
                continue;
            };
            if !scopes.get(key).is_some_and(|s| s == tab) || !placed.insert(format!("{tab}:{key}"))
            {
                continue;
            }
            rows.push(Row {
                tab: tab.into(),
                page: esc(&page),
                group: esc(&group),
                label: esc(&line_label),
                key: key.clone(),
                source: format!("orca:{orca_key}"),
            });
        }
        // Canonical options Orca's tabs do not show: placed by their PrusaSlicer category.
        for (key, scope) in &scopes {
            if scope != tab || is_bookkeeping(key) || placed.contains(&format!("{tab}:{key}")) {
                continue;
            }
            let category = prusa_defs
                .get(key)
                .and_then(|d| d.category.as_deref())
                .unwrap_or("");
            let page = fallback_page(tab, key, category);
            rows.push(Row {
                tab: tab.into(),
                page: page.into(),
                group: "More settings".into(),
                label: String::new(),
                key: key.clone(),
                source: "prusa".into(),
            });
        }
    }
    regroup(&mut rows);
    for r in &rows {
        pages.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\n",
            r.tab, r.page, r.group, r.label, r.key, r.source
        ));
    }

    for (path, content) in [(TEXT_OUTPUT, text), (PAGES_OUTPUT, pages)] {
        let path = root.join(path);
        if let Err(e) =
            fs::create_dir_all(path.parent().unwrap()).and_then(|()| fs::write(&path, content))
        {
            eprintln!("{}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    }
    println!("wrote {TEXT_OUTPUT} and {PAGES_OUTPUT}");
    ExitCode::SUCCESS
}

/// `cargo xtask gen-setting-names`: rewrites docs/setting-names.md from the GUI's tables. The
/// generator is the mica-gui test that checks the file (`settings::names`), run in write mode.
pub fn setting_names(root: &Path) -> ExitCode {
    let profile = crate::profile();
    let status = std::process::Command::new("cargo")
        .args([
            "test",
            "--offline",
            "--profile",
            &profile,
            "-p",
            "mica-gui",
            "--",
            "--exact",
            "settings::names::tests::setting_names_doc_is_current",
        ])
        .env("MICA_WRITE_SETTING_NAMES", "1")
        .current_dir(root)
        .status();
    if status.is_ok_and(|s| s.success()) {
        println!("wrote docs/setting-names.md");
        ExitCode::SUCCESS
    } else {
        eprintln!("could not generate docs/setting-names.md");
        ExitCode::FAILURE
    }
}

/// One row of `pages.tsv`.
#[derive(Clone, Debug)]
struct Row {
    tab: String,
    page: String,
    group: String,
    label: String,
    key: String,
    source: String,
}

/// Setting groups with one Mica name whose contents depend on the slicing rules
/// (docs/setting-names.md): `(tab, page, group, keys)`. The rules-based visibility
/// (`settings::applies`) shows each rule set's own controls. A listed key moves into the group,
/// in this order, after the group's rows from Orca's layout; a group Orca's layout lacks opens
/// after the page's own groups.
const RULE_GROUPS: &[(&str, &str, &str, &[&str])] = &[
    (
        "printer",
        "Extruder",
        "Travel lift",
        &[
            "z_hop_types",
            "retract_lift",
            "travel_slope",
            "travel_ramping_lift",
            "travel_max_lift",
            "travel_lift_before_obstacle",
            "retract_lift_above",
            "retract_lift_below",
        ],
    ),
    (
        "filament",
        "Advanced",
        "Travel lift",
        &[
            "filament_retract_lift",
            "filament_travel_slope",
            "filament_travel_ramping_lift",
            "filament_travel_max_lift",
            "filament_travel_lift_before_obstacle",
            "filament_retract_lift_above",
            "filament_retract_lift_below",
        ],
    ),
    (
        "print",
        "Quality",
        "Bridges",
        &[
            "bridge_flow_ratio",
            "bridge_density",
            "internal_bridge_density",
            "bridge_line_width",
            "thick_bridges",
            "enable_extra_bridge_layer",
        ],
    ),
    (
        "filament",
        "Cooling",
        "Cooling slowdown",
        &[
            "cooling",
            "min_print_speed",
            "cooling_slowdown_logic",
            "cooling_perimeter_transition_distance",
        ],
    ),
];

/// Orca's group names Mica replaces with a [`RULE_GROUPS`] name: `(tab, page, Orca group, Mica group)`.
const RULE_GROUP_RENAMES: &[(&str, &str, &str, &str)] = &[
    ("printer", "Extruder", "Z-Hop", "Travel lift"),
    ("print", "Quality", "Bridging", "Bridges"),
];

fn regroup(rows: &mut Vec<Row>) {
    for (tab, page, from, to) in RULE_GROUP_RENAMES {
        for r in rows.iter_mut() {
            if r.tab == *tab && r.page == *page && r.group == *from {
                r.group = (*to).into();
            }
        }
    }
    for (tab, page, group, keys) in RULE_GROUPS {
        let mut moved = Vec::new();
        for key in *keys {
            if let Some(i) = rows.iter().position(|r| r.tab == *tab && r.key == *key) {
                let mut r = rows.remove(i);
                r.page = (*page).into();
                r.group = (*group).into();
                r.label.clear();
                moved.push(r);
            }
        }
        let same = |r: &Row, g: Option<&str>| {
            r.tab == *tab && r.page == *page && g.is_none_or(|g| r.group == g)
        };
        // Where the group's rows from Orca's layout started, else after the page's own groups
        // (before its "More settings" fallbacks), else where the page starts.
        let at = rows
            .iter()
            .position(|r| same(r, Some(group)))
            .or_else(|| {
                rows.iter()
                    .rposition(|r| same(r, None) && r.group != "More settings")
                    .map(|i| i + 1)
            })
            .or_else(|| rows.iter().position(|r| same(r, None)))
            .unwrap_or(rows.len());
        // Rows of the group Orca's layout left in place (none of them listed) go first.
        let mut rest = Vec::new();
        while at < rows.len() && same(&rows[at], Some(group)) {
            rest.push(rows.remove(at));
        }
        for (n, r) in rest.into_iter().chain(moved).enumerate() {
            rows.insert(at + n, r);
        }
    }
}

fn is_bookkeeping(key: &str) -> bool {
    key.ends_with("_settings_id")
        || key.ends_with("_cummulative")
        || key.starts_with("compatible_")
        || key.starts_with("printhost_")
        || key.starts_with("print_host")
        || matches!(
            key,
            "inherits"
                | "renamed_from"
                | "alias"
                | "printer_vendor"
                | "default_print_profile"
                | "host_type"
                | "printer_model"
                | "printer_variant"
                | "printer_technology"
                | "default_filament_profile"
                | "printer_notes"
                | "wiping_volumes_matrix"
                | "wiping_volumes_use_custom_matrix"
        )
}

/// Orca's tab pages for a canonical option Orca's layout does not place, chosen by the
/// PrusaSlicer category and name.
fn fallback_page(tab: &str, key: &str, category: &str) -> &'static str {
    match tab {
        "print" => match category {
            _ if key.contains("speed") || key.contains("acceleration") => "Speed",
            _ if key.ends_with("_extruder") || key.contains("wipe_tower") => "Multimaterial",
            "Layers and Perimeters" | "Extrusion Width" | "Advanced" => "Quality",
            _ if key.contains("ironing") => "Quality",
            "Infill" => "Strength",
            "Speed" => "Speed",
            "Support material" => "Support",
            "Multiple Extruders" | "Wipe tower" => "Multimaterial",
            _ => "Others",
        },
        "filament" => {
            if key.contains("fan") || key.contains("cooling") || key.contains("slowdown") {
                "Cooling"
            } else if key.contains("gcode")
                || key.contains("ramming")
                || key.contains("purge")
                || key.contains("wipe_tower")
                || key.contains("multitool")
                || key.contains("toolchange")
                || key.contains("loading")
                || key.contains("unload")
            {
                "Multimaterial"
            } else if key.contains("temperature")
                || key.contains("diameter")
                || key.contains("density")
                || key.contains("cost")
                || key.contains("colour")
                || key.contains("extrusion_multiplier")
                || key.contains("filament_type")
            {
                "Filament"
            } else {
                "Advanced"
            }
        }
        _ => {
            if key.contains("gcode") {
                "Machine G-code"
            } else if key.starts_with("machine_") {
                "Motion ability"
            } else if key.contains("retract")
                || key.contains("wipe")
                || key.contains("lift")
                || key.contains("nozzle")
                || key.contains("travel")
                || key.contains("extruder")
                || key.contains("layer_height")
            {
                "Extruder"
            } else if key.contains("wipe_tower") || key.contains("multi_material") {
                "Multimaterial"
            } else {
                "Basic information"
            }
        }
    }
}

fn synthesize_label(key: &str) -> String {
    if let Some(rest) = key.strip_prefix("machine_max_")
        && let Some((what, axis)) = rest.rsplit_once('_')
        && axis.len() == 1
    {
        let what = if what == "feedrate" { "speed" } else { what };
        return format!("Maximum {what} {}", axis.to_ascii_uppercase());
    }
    let words: Vec<String> = key
        .split('_')
        .map(|w| match w {
            "x" | "y" | "z" | "e" => w.to_ascii_uppercase(),
            "gcode" => "G-code".into(),
            _ => w.to_string(),
        })
        .collect();
    let mut s = words.join(" ");
    if let Some(first) = s.get(..1) {
        let upper = first.to_ascii_uppercase();
        s.replace_range(..1, &upper);
    }
    s
}

/// All `def = this->add("key", …)` / `add_nullable` blocks of an upstream `PrintConfig.cpp`.
fn parse_defs(cpp: &str) -> BTreeMap<String, Def> {
    let mut out: BTreeMap<String, Def> = BTreeMap::new();
    let mut starts: Vec<(usize, String)> = Vec::new();
    for marker in ["this->add(\"", "this->add_nullable(\""] {
        let mut from = 0;
        while let Some(i) = cpp[from..].find(marker) {
            let at = from + i;
            let rest = &cpp[at + marker.len()..];
            if let Some(end) = rest.find('"') {
                starts.push((at, rest[..end].to_string()));
            }
            from = at + marker.len();
        }
    }
    starts.sort();
    for (n, (at, key)) in starts.iter().enumerate() {
        let end = starts.get(n + 1).map_or(cpp.len(), |(e, _)| *e);
        // A block also ends where `def` is next reassigned: code after the last literal
        // definition of a section (loops defining derived keys with their own min/max) is
        // not part of it.
        let end = cpp[*at..end]
            .match_indices("def = ")
            .find(|(j, _)| {
                // `def = …`, not `def->enum_def = …` or `def_top_fill_pattern = def = …`.
                cpp[..*at + j]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_whitespace)
            })
            .map_or(end, |(j, _)| *at + j);
        let block = &cpp[*at..end];
        let def = parse_block(block);
        // First definition wins (FFF parameters precede SLA and CLI ones).
        out.entry(key.clone()).or_insert(def);
    }
    out
}

fn parse_block(block: &str) -> Def {
    let mut def = Def::default();
    let field = |name: &str| -> Option<String> {
        let marker = format!("def->{name}");
        let mut from = 0;
        while let Some(i) = block[from..].find(&marker) {
            let after = &block[from + i + marker.len()..];
            let trimmed = after.trim_start();
            if let Some(rhs) = trimmed.strip_prefix('=') {
                return cpp_string(rhs);
            }
            from += i + marker.len();
        }
        None
    };
    def.label = field("label");
    def.full_label = field("full_label");
    def.category = field("category");
    def.tooltip = field("tooltip");
    def.sidetext = field("sidetext");
    let number = |name: &str| -> Option<String> {
        // The last assignment wins, as in C++: PrusaSlicer's `first_layer_bed_temperature`
        // says `def->max = 0; def->max = 300;`, and reading the first made the GUI clamp the
        // first-layer bed temperature to 0 °C.
        let marker = format!("def->{name} ");
        let i = block.rfind(&marker).or_else(|| {
            let marker = format!("def->{name}=");
            block.rfind(&marker)
        })?;
        let marker = format!("def->{name}");
        let rhs = block[i + marker.len()..].trim_start().strip_prefix('=')?;
        let value: String = rhs
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit() || matches!(c, '-' | '.' | 'e' | 'E' | '+'))
            .collect();
        let value = value.trim_end_matches(['e', 'E', '+']).to_string();
        value.parse::<f64>().ok().map(|_| {
            let t = value.trim_end_matches('.');
            t.to_string()
        })
    };
    def.min = number("min");
    def.max = number("max");
    def.mode = block.find("def->mode").map(|i| {
        let rhs = block[i..].split('=').nth(1).unwrap_or("");
        rhs.split(';').next().unwrap_or("").trim().to_string()
    });
    def.enums = parse_enums(block);
    def.enum_from = block
        .find("->enum_def->clone()")
        .or_else(|| block.find("->enum_values;"))
        .and_then(|i| {
            let before = &block[..i];
            let start = before.rfind("def_")?;
            Some(before[start + 4..].to_string())
        });
    def
}

/// Enum (value, label) pairs from PrusaSlicer's `set_enum<T>({ {"v", L("Label")}, … })` /
/// `set_enum_values(…, { … })` / `set_enum_values({"a", "b"})` and Orca's
/// `enum_values.push_back("v")` + `enum_labels.push_back(L("Label"))`.
fn parse_enums(block: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for marker in ["set_enum<", "set_enum_values(", "set_enum_labels("] {
        let Some(i) = block.find(marker) else {
            continue;
        };
        let Some(open) = block[i..].find('{') else {
            continue;
        };
        let body = &block[i + open + 1..];
        let Some(close) = body.find("});") else {
            continue;
        };
        let body = &body[..close];
        if body.contains('{') {
            for pair in body.split('{').skip(1) {
                let pair = pair.split('}').next().unwrap_or("");
                let mut parts = pair.splitn(2, ',');
                let v = parts.next().and_then(cpp_string);
                let l = parts.next().and_then(cpp_string);
                if let (Some(v), Some(l)) = (v, l) {
                    out.push((v, l));
                }
            }
        } else {
            for item in body.split(',') {
                if let Some(v) = cpp_string(item) {
                    out.push((v.clone(), v));
                }
            }
        }
        if !out.is_empty() {
            return out;
        }
    }
    let values: Vec<String> = block
        .match_indices("enum_values.push_back(")
        .chain(block.match_indices("enum_values.emplace_back("))
        .filter_map(|(i, m)| cpp_string(&block[i + m.len()..]))
        .collect();
    let labels: Vec<String> = block
        .match_indices("enum_labels.push_back(")
        .chain(block.match_indices("enum_labels.emplace_back("))
        .filter_map(|(i, m)| cpp_string(&block[i + m.len()..]))
        .collect();
    values
        .into_iter()
        .enumerate()
        .map(|(i, v)| {
            let l = labels.get(i).cloned().unwrap_or_else(|| v.clone());
            (v, l)
        })
        .collect()
}

fn resolve_enums(def: &Def, all: &BTreeMap<String, Def>) -> Vec<(String, String)> {
    if !def.enums.is_empty() {
        return def.enums.clone();
    }
    def.enum_from
        .as_ref()
        .and_then(|k| all.get(k))
        .map(|d| d.enums.clone())
        .unwrap_or_default()
}

/// One C++ string expression: optional `L(` / `_L(` / `L_CONTEXT(` / `_(` wrapper, then one or
/// more adjacent literals. Returns `None` for anything else (formats, variables).
fn cpp_string(expr: &str) -> Option<String> {
    let mut s = expr.trim_start();
    for wrapper in ["L_CONTEXT(", "_L(", "L(", "_("] {
        if let Some(rest) = s.strip_prefix(wrapper) {
            s = rest.trim_start();
            break;
        }
    }
    if !s.starts_with('"') {
        return None;
    }
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    loop {
        // Skip whitespace between adjacent literals.
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.peek() != Some(&'"') {
            break;
        }
        chars.next();
        loop {
            match chars.next()? {
                '"' => break,
                '\\' => match chars.next()? {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => {}
                    other => out.push(other),
                },
                c => out.push(c),
            }
        }
    }
    Some(out)
}

/// Body of the first function whose definition line starts with `signature` (column 0) up to
/// the next definition at column 0.
fn function_body<'a>(cpp: &'a str, signature: &str) -> Option<&'a str> {
    let start = cpp
        .match_indices(signature)
        .find(|(i, _)| *i == 0 || cpp.as_bytes()[i - 1] == b'\n')
        .map(|(i, _)| i)?;
    let rest = &cpp[start + signature.len()..];
    let mut end = rest.len();
    for (i, line) in rest.match_indices('\n') {
        let _ = line;
        let next = &rest[i + 1..];
        if next
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() && !next.starts_with("else"))
            && (next.starts_with("void ")
                || next.starts_with("PageShp ")
                || next.starts_with("bool ")
                || next.starts_with("wxString ")
                || next.starts_with("static ")
                || next.starts_with("Page")
                || next.starts_with("std::"))
        {
            end = i;
            break;
        }
    }
    Some(&rest[..end])
}

/// (page, group, line label, option key) in order of appearance.
fn parse_layout(body: &str) -> Vec<(String, String, String, String)> {
    let mut out = Vec::new();
    let mut page = String::new();
    let mut group = String::new();
    let mut line_label = String::new();
    let mut last_option: Option<String> = None;
    let quoted = |s: &str, after: &str| -> Option<String> {
        let i = s.find(after)? + after.len();
        let rest = &s[i..];
        let rest = rest.trim_start();
        let rest = rest.strip_prefix('"')?;
        Some(rest[..rest.find('"')?].to_string())
    };
    for raw in body.lines() {
        let line = raw.trim();
        if line.starts_with("//") {
            continue;
        }
        if let Some(i) = line.find("add_options_page(") {
            // Per-extruder pages are named at runtime ("Extruder 1", …).
            page = cpp_string(&line[i + "add_options_page(".len()..])
                .unwrap_or_else(|| "Extruder".to_string());
            group.clear();
            continue;
        }
        if let Some(i) = line.find("new_optgroup(") {
            group = cpp_string(&line[i + "new_optgroup(".len()..]).unwrap_or_default();
            line_label.clear();
            continue;
        }
        if line.contains("Line line")
            || line.starts_with("line = {")
            || line.contains("_line = {")
            || line.contains("Line{")
        {
            if let Some(i) = line.find('{') {
                line_label = cpp_string(&line[i + 1..]).unwrap_or_default();
            }
            continue;
        }
        if line.contains("append_line(") {
            line_label.clear();
            continue;
        }
        if line.contains("append_option(") {
            if let Some(key) = quoted(line, "get_option(") {
                out.push((page.clone(), group.clone(), line_label.clone(), key));
            } else if let Some(key) = last_option.take() {
                out.push((page.clone(), group.clone(), line_label.clone(), key));
            }
            continue;
        }
        if let Some(key) = quoted(line, "get_option(") {
            last_option = Some(key);
            continue;
        }
        if let Some(key) = quoted(line, "append_single_option_line(") {
            out.push((page.clone(), group.clone(), String::new(), key));
            continue;
        }
        if line.contains("append_single_option_line(") {
            if let Some(key) = last_option.take() {
                out.push((page.clone(), group.clone(), String::new(), key));
            }
            continue;
        }
        if let Some(i) = line.find("append_option_line(optgroup, ") {
            let rest = &line[i + "append_option_line(optgroup, ".len()..];
            if let Some(prefix) = cpp_string(rest) {
                if rest.contains("+ axis") {
                    for axis in ["x", "y", "z", "e"] {
                        out.push((
                            page.clone(),
                            group.clone(),
                            String::new(),
                            format!("{prefix}{axis}"),
                        ));
                    }
                } else {
                    out.push((page.clone(), group.clone(), String::new(), prefix));
                }
            } else if rest.starts_with("speed_axis") {
                for axis in ["x", "y", "z", "e"] {
                    out.push((
                        page.clone(),
                        group.clone(),
                        String::new(),
                        format!("machine_max_speed_{axis}"),
                    ));
                }
            }
        }
    }
    out
}

/// Pattern names `mica_core::fill::Pattern::from_config_name` accepts (read from its source so the
/// GUI lists stay in step with the engine without a crate dependency).
fn supported_fill_patterns() -> Vec<String> {
    let src = include_str!("../../crates/mica-core/src/fill/mod.rs");
    let start = src.find("fn from_config_name").unwrap_or(0);
    let body = &src[start..];
    let body = &body[..body.find("_ => return None").unwrap_or(body.len())];
    body.lines()
        .filter_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix('"')?;
            let end = rest.find('"')?;
            l.contains("=>").then(|| rest[..end].to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_and_blocks() {
        assert_eq!(
            cpp_string(" L(\"Number of walls \"\n   \"of every layer.\");"),
            Some("Number of walls of every layer.".into())
        );
        assert_eq!(
            cpp_string("L_CONTEXT(\"Bottom\", \"Layers\")"),
            Some("Bottom".into())
        );
        assert_eq!(cpp_string("(boost::format(\"x\"))"), None);
        let defs = parse_defs(
            "    def = this->add(\"wall_loops\", coInt);\n    def->label = L(\"Wall loops\");\n    def->category = L(\"Strength\");\n    def->tooltip = L(\"Number of walls.\");\n    def->min = 0;\n    def->max = 1000;\n    def->set_default_value(new ConfigOptionInt(2));\n    def = this->add(\"p\", coEnum);\n    def->mode = comAdvanced;\n    def->set_enum<SeamPosition>({\n        { \"random\",  L(\"Random\") },\n        { \"nearest\", L(\"Nearest\") }\n    });\n",
        );
        let w = &defs["wall_loops"];
        assert_eq!(w.label.as_deref(), Some("Wall loops"));
        assert_eq!(w.min.as_deref(), Some("0"));
        assert_eq!(w.max.as_deref(), Some("1000"));
        assert_eq!(w.mode, None);
        let p = &defs["p"];
        assert_eq!(p.mode.as_deref(), Some("comAdvanced"));
        assert_eq!(p.enums.len(), 2);
        assert_eq!(p.enums[1], ("nearest".into(), "Nearest".into()));
    }

    #[test]
    fn layout_lines() {
        let body = "{\n    auto page = add_options_page(L(\"Quality\"), \"x\");\n    auto optgroup = page->new_optgroup(L(\"Layer height\"), L\"p\");\n    optgroup->append_single_option_line(\"layer_height\",\"q\");\n    Line line = { L(\"Solid layers\"), \"\" };\n    line.append_option(optgroup->get_option(\"top_shell_layers\"));\n    optgroup->append_line(line);\n    Option option = optgroup->get_option(\"notes\");\n    optgroup->append_single_option_line(option);\n";
        let lines = parse_layout(body);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].0, "Quality");
        assert_eq!(lines[0].3, "layer_height");
        assert_eq!(lines[1].2, "Solid layers");
        assert_eq!(lines[2].3, "notes");
    }

    /// PrusaSlicer's `first_layer_bed_temperature` assigns `def->max` twice (0, then 300):
    /// reading the first clamped the GUI field to 0 °C. Code after a section's last literal
    /// definition (loops adding derived keys) gave `min_bead_width` a stray maximum.
    #[test]
    fn last_assignment_wins_and_blocks_end_at_the_next_def() {
        let cpp = "    def = this->add(\"first_layer_bed_temperature\", coInts);\n    def->max = 0;\n    def->max = 300;\n    def->set_default_value(new ConfigOptionInts { 0 });\n\n    def = this->add(\"min_bead_width\", coPercent);\n    def->enum_def = nullptr;\n    def->min = 0;\n\n    for (auto& k : keys) {\n        def = this->add_nullable(k, t);\n        def->max = 20;\n    }\n";
        let defs = parse_defs(cpp);
        assert_eq!(
            defs["first_layer_bed_temperature"].max.as_deref(),
            Some("300")
        );
        assert_eq!(defs["first_layer_bed_temperature"].min, None);
        assert_eq!(defs["min_bead_width"].min.as_deref(), Some("0"));
        assert_eq!(defs["min_bead_width"].max, None);
    }
}
