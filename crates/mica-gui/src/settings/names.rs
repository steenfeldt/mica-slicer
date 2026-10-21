// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! `docs/setting-names.md`: every setting's Mica name next to its PrusaSlicer and OrcaSlicer
//! names, which slicing rules use it, and whether it is merged, rule-specific or Mica's own.
//!
//! Generated from the GUI's own tables (labels, pages, `applies`), the Orca option mapping
//! (through the `source` column of `option_text.tsv`) and `mica_core::merged`, so it cannot
//! drift: [`tests::setting_names_doc_is_current`] fails when the file is stale, and
//! `cargo xtask gen-setting-names` (also run by `cargo xtask gen-option-text`) rewrites it.

use super::unimplemented::{self, Hidden};
use super::{Tab, applies, merged_away, pages, text};
use mica_core::config::{option_defs, orca_table};
use mica_core::merged;
use mica_core::rules::Rules;

/// Setting groups with one name whose contents follow the slicing rules (the generator's
/// `RULE_GROUPS`).
const RULE_GROUPS: &[&str] = &["Travel lift", "Bridges", "Cooling slowdown"];

/// Orca keys that are not Mica options (no row of their own), with OrcaSlicer's label.
const ORCA_ONLY_LABELS: &[(&str, &str)] = &[("exclude_object", "Exclude objects")];

fn cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

fn code(key: &str) -> String {
    if key.is_empty() {
        "—".into()
    } else {
        format!("`{key}`")
    }
}

/// `(Orca keys with labels)` for a Mica key.
fn orca_names(key: &str) -> Vec<(String, String)> {
    let t = text();
    let label = |k: &str| {
        t.get(k)
            .map(|x| x.orca_label.clone())
            .filter(|l| !l.is_empty())
            .or_else(|| {
                ORCA_ONLY_LABELS
                    .iter()
                    .find(|(o, _)| *o == k)
                    .map(|(_, l)| l.to_string())
            })
            .unwrap_or_default()
    };
    if let Some((_, keys, _)) = merged::PRUSA_KEYED.iter().find(|(k, _, _)| *k == key) {
        let own = t.get(key).map(|x| x.orca_label.clone()).unwrap_or_default();
        return keys
            .iter()
            .enumerate()
            .map(|(i, k)| {
                let l = if i == 0 && !own.is_empty() {
                    own.clone()
                } else {
                    label(k)
                };
                (k.to_string(), l)
            })
            .collect();
    }
    let source = t.get(key).map(|x| x.source.as_str()).unwrap_or("");
    let orca_key = source
        .strip_prefix("orca:")
        .or_else(|| source.strip_prefix("orca-label:"));
    match orca_key {
        Some(k) => vec![(k.to_string(), label(key))],
        // Orca's own options without a generated row (hand-written text).
        None if orca_table(key).is_some() => vec![(key.to_string(), label(key))],
        None => Vec::new(),
    }
}

/// PrusaSlicer's key and label for a Mica key (empty if PrusaSlicer has none).
fn prusa_name(key: &str) -> (String, String) {
    let t = text();
    let prusa = match merged::by_orca(key) {
        Some(p) => p.prusa,
        None if orca_table(key).is_none() && !key.starts_with("mica_") => key,
        None => return Default::default(),
    };
    let label = t
        .get(prusa)
        .map(|x| x.prusa_label.clone())
        .unwrap_or_default();
    (prusa.to_string(), label)
}

fn rules_of(key: &str) -> &'static str {
    match (
        applies(key, Rules::PrusaSlicer),
        applies(key, Rules::OrcaSlicer),
    ) {
        (true, true) => "both",
        (true, false) => "PrusaSlicer",
        (false, true) => "OrcaSlicer",
        (false, false) => "—",
    }
}

fn note(key: &str, group: &str, prusa: &str, orca: &[(String, String)]) -> String {
    if let Some(p) = merged::by_orca(key) {
        return format!("merged with PrusaSlicer `{}`: {}", p.prusa, p.note);
    }
    if let Some((_, _, n)) = merged::PRUSA_KEYED.iter().find(|(k, _, _)| *k == key) {
        return format!("merged: {n}");
    }
    let mut notes = Vec::new();
    if key.starts_with("mica_") {
        notes.push("Mica-only".to_string());
    } else if prusa.is_empty() {
        notes.push("OrcaSlicer only".into());
    } else if orca.is_empty() {
        notes.push("PrusaSlicer only".into());
    }
    if RULE_GROUPS.contains(&group) && rules_of(key) != "both" {
        notes.push(format!("rule-specific ({group})"));
    }
    if unimplemented::setting(key).is_some_and(|s| s.hidden == Hidden::Klipper) {
        notes.push("hidden on Klipper printers (not implemented there)".into());
    }
    let choices: Vec<String> = unimplemented::CHOICES
        .iter()
        .filter(|c| c.key == key)
        .map(|c| {
            format!(
                "{}{}",
                unimplemented::choice_label(key, c.value),
                if c.hidden == Hidden::Klipper {
                    " on Klipper"
                } else {
                    ""
                }
            )
        })
        .collect();
    if !choices.is_empty() {
        notes.push(format!("not offered: {}", choices.join(", ")));
    }
    notes.join("; ")
}

/// The whole document.
pub fn markdown() -> String {
    let t = text();
    let mut out = String::new();
    out.push_str(
        "<!-- Generated by `cargo xtask gen-setting-names` (crates/mica-gui/src/settings/names.rs). Do not edit. -->\n\n\
         # Setting names\n\n\
         Every setting the editor shows, with its name in Mica, PrusaSlicer and OrcaSlicer. Mica's label is\n\
         OrcaSlicer's where Orca has the option (the GUI follows Orca's layout), else PrusaSlicer's.\n\n\
         - **Rules**: under which slicing rules (`mica_slicing_rules`, see slicing-rules.md) the row shows: `both`,\n  \
         `PrusaSlicer` or `OrcaSlicer`.\n\
         - **Merged**: one row on every printer for an option both slicers have under different names. The\n  \
         Orca key is Mica's (the PrusaSlicer counterpart is hidden); PrusaSlicer files are translated on\n  \
         import and get the counterpart written back on export (`mica_core::merged`).\n\
         - **Rule-specific**: a group with one name (Travel lift, Bridges, Cooling slowdown) whose controls\n  \
         differ by rules; the row shows only under the rules named.\n\
         - **Mica-only**: Mica's own options (`mica_*`). **OrcaSlicer only** / **PrusaSlicer only**: no\n  \
         counterpart in the other slicer.\n\n",
    );
    out.push_str("## Merged settings\n\n| Mica | PrusaSlicer | OrcaSlicer | Translation |\n|---|---|---|---|\n");
    for p in merged::PAIRS {
        let (_, pl) = prusa_name(p.orca);
        let orca = orca_names(p.orca);
        out.push_str(&format!(
            "| {} (`{}`) | {} (`{}`) | {} | {} |\n",
            cell(&super::label(p.orca)),
            p.orca,
            cell(&pl),
            p.prusa,
            orca.iter()
                .map(|(k, l)| format!("{} (`{k}`)", cell(l)))
                .collect::<Vec<_>>()
                .join(", "),
            cell(p.note)
        ));
    }
    for (key, _, n) in merged::PRUSA_KEYED {
        let (_, pl) = prusa_name(key);
        out.push_str(&format!(
            "| {} (`{key}`) | {} (`{key}`) | {} | {} |\n",
            cell(&super::label(key)),
            cell(&pl),
            orca_names(key)
                .iter()
                .map(|(k, l)| format!("{} (`{k}`)", cell(l)))
                .collect::<Vec<_>>()
                .join(", "),
            cell(n)
        ));
    }
    out.push_str("\n## Rule-specific groups\n\n| Group | PrusaSlicer rules | OrcaSlicer rules | Both |\n|---|---|---|---|\n");
    for group in RULE_GROUPS {
        let mut cols: [Vec<String>; 3] = Default::default();
        for tab in [Tab::Print, Tab::Filament, Tab::Printer] {
            for p in pages(tab) {
                for g in p.groups.iter().filter(|g| g.name == *group) {
                    for k in g.lines.iter().flat_map(|l| &l.keys) {
                        let i = match rules_of(k) {
                            "PrusaSlicer" => 0,
                            "OrcaSlicer" => 1,
                            _ => 2,
                        };
                        cols[i].push(format!("{} (`{k}`)", cell(&super::label(k))));
                    }
                }
            }
        }
        out.push_str(&format!(
            "| {group} | {} | {} | {} |\n",
            cols[0].join(", "),
            cols[1].join(", "),
            cols[2].join(", ")
        ));
    }
    for (tab, title) in [
        (Tab::Print, "Process"),
        (Tab::Filament, "Filament"),
        (Tab::Printer, "Printer"),
    ] {
        out.push_str(&format!(
            "\n## {title} settings\n\n| Page › group | Mica label | Mica key | PrusaSlicer key | PrusaSlicer label | OrcaSlicer key | OrcaSlicer label | Rules | Notes |\n|---|---|---|---|---|---|---|---|---|\n"
        ));
        for p in pages(tab) {
            for g in &p.groups {
                for key in g.lines.iter().flat_map(|l| &l.keys) {
                    if merged_away(key)
                        || !option_defs().contains_key(key)
                        || unimplemented::hidden_everywhere(key)
                    {
                        continue;
                    }
                    let (pk, pl) = prusa_name(key);
                    let orca = orca_names(key);
                    out.push_str(&format!(
                        "| {} › {} | {} | `{key}` | {} | {} | {} | {} | {} | {} |\n",
                        cell(&p.name),
                        cell(&g.name),
                        cell(
                            &t.get(key)
                                .map_or_else(|| key.to_string(), |x| x.label.clone())
                        ),
                        code(&pk),
                        cell(&pl),
                        if orca.is_empty() {
                            "—".to_string()
                        } else {
                            orca.iter()
                                .map(|(k, _)| code(k))
                                .collect::<Vec<_>>()
                                .join(", ")
                        },
                        orca.iter()
                            .map(|(_, l)| cell(l))
                            .collect::<Vec<_>>()
                            .join(", "),
                        rules_of(key),
                        cell(&note(key, &g.name, &pk, &orca)),
                    ));
                }
            }
        }
    }
    out.push_str(
        "\n## Hidden PrusaSlicer counterparts\n\nThese PrusaSlicer options never show: the merged Orca row above stands for them.\n\n| PrusaSlicer key | PrusaSlicer label | Shown as |\n|---|---|---|\n",
    );
    for p in merged::PAIRS {
        let (_, pl) = prusa_name(p.orca);
        out.push_str(&format!(
            "| `{}` | {} | {} (`{}`) |\n",
            p.prusa,
            cell(&pl),
            cell(&super::label(p.orca)),
            p.orca
        ));
    }
    out.push_str(
        "\n## Hidden: not implemented by Mica\n\nThese options are never shown because Mica's engine does not implement them. Profiles and \
         projects keep their values (loaded and saved unchanged); a value that would change the \
         G-code in the upstream slicer is reported when the profile is used.\n\n\
         | Key | Label | Mica prints as |\n|---|---|---|\n",
    );
    for s in unimplemented::SETTINGS
        .iter()
        .filter(|s| s.hidden == Hidden::Everywhere)
    {
        out.push_str(&format!(
            "| `{}` | {} | {} |\n",
            s.key,
            cell(&super::label(s.key)),
            code(s.inert)
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/setting-names.md");

    /// `docs/setting-names.md` matches the tables; `MICA_WRITE_SETTING_NAMES=1` rewrites it
    /// (`cargo xtask gen-setting-names`).
    #[test]
    fn setting_names_doc_is_current() {
        let doc = markdown();
        if std::env::var_os("MICA_WRITE_SETTING_NAMES").is_some() {
            std::fs::write(DOC, &doc).unwrap();
            return;
        }
        let on_disk = std::fs::read_to_string(DOC).unwrap_or_default();
        assert!(
            on_disk == doc,
            "docs/setting-names.md is stale: run `cargo xtask gen-setting-names`"
        );
    }

    #[test]
    fn merged_settings_show_once_on_every_printer() {
        let doc = markdown();
        for p in merged::PAIRS {
            assert!(applies(p.orca, Rules::PrusaSlicer) && applies(p.orca, Rules::OrcaSlicer));
            assert!(!applies(p.prusa, Rules::PrusaSlicer) && !applies(p.prusa, Rules::OrcaSlicer));
            assert!(doc.contains(&format!("| `{}` |", p.orca)), "{}", p.orca);
        }
        // Top surface walls: one row (PrusaSlicer's key, Orca's label).
        let rows = |k: &str| doc.matches(&format!("| `{k}` |")).count();
        assert_eq!(rows("top_one_perimeter_type"), 1);
        assert_eq!(rows("gcode_label_objects"), 1);
        assert!(doc.contains("`exclude_object`"));
    }

    /// Orca's filament overrides (`filament_retract_when_changing_layer` over the printer's
    /// `retract_when_changing_layer`) name Mica's filament keys, so neither the printer option
    /// nor its filament override reads as PrusaSlicer-only.
    #[test]
    fn retraction_options_name_their_orca_counterparts() {
        for (key, orca) in [
            ("retract_layer_change", "retract_when_changing_layer"),
            (
                "filament_retract_layer_change",
                "filament_retract_when_changing_layer",
            ),
            ("retract_lift", "z_hop"),
            ("filament_retract_lift", "filament_z_hop"),
            ("wipe", "wipe"),
            ("filament_wipe", "filament_wipe"),
        ] {
            let names = orca_names(key);
            assert_eq!(names.first().map(|(k, _)| k.as_str()), Some(orca), "{key}");
            assert!(!names[0].1.is_empty(), "{key}: no Orca label");
            assert_eq!(rules_of(key), "both", "{key}");
            assert_eq!(note(key, "Retraction", key, &names), "", "{key}");
        }
    }
}
