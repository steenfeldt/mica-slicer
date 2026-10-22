// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/Format/bbs_3mf.cpp :: _BBS_3MF_Importer::_handle_end_metadata
// Source: orca@8500fcdcca src/libslic3r/Format/bbs_3mf.cpp :: _BBS_3MF_Exporter::_add_model_config_file_to_archive
// Source: orca@8500fcdcca src/slic3r/GUI/PartPlate.cpp :: PartPlate::set_bed_type
// Source: orca@8500fcdcca src/slic3r/GUI/PlateSettingsDialog.cpp :: PlateSettingsDialog::PlateSettingsDialog
// Changes: Only the plate keys and their values (bed_type, print_sequence, spiral_mode as Orca writes them in model_settings.config; "same as global" = key absent), mapped onto Mica's canonical option names; any other plate override travels as a mica_override: key Orca ignores; spiral vase is offered for Orca-rules printers. Original Rust.

//! A plate's own settings, as OrcaSlicer's plate settings dialog has them: each plate may
//! override plate-level options (bed type, print sequence, spiral vase…) and, in Mica, any
//! other print setting. An absent key means "same as the global settings", as in Orca, where
//! `PartPlate::set_bed_type(btDefault)` erases the key.
//!
//! The overrides are `Plate::overrides` (canonical option names, plus `curr_bed_type`) and
//! apply on top of the global configuration when that plate is sliced
//! (`App::plate_config`). In a project they are plate metadata: Orca's own keys for what Orca
//! has (so OrcaSlicer reads them natively) and `mica_override:KEY` for the rest, which Orca's
//! importer ignores; the Prusa flavour keeps the plates in Mica's marker (`mica.json`) with the
//! same metadata.

use std::collections::BTreeMap;

pub type Settings = BTreeMap<String, String>;

/// The plate key of the bed type (Orca's config key; the 3MF spells it `bed_type`).
pub const BED_TYPE: &str = "curr_bed_type";

/// Prefix of a plate override Orca has no plate key for.
const PREFIX: &str = "mica_override:";

/// Options offered for a plate, in the order of Orca's plate settings dialog. The bed type
/// and spiral vase (Orca's plate `spiral_mode`) are only offered on printers with OrcaSlicer
/// rules (`offered`).
pub const PLATE_OPTIONS: [&str; 3] = [BED_TYPE, "complete_objects", SPIRAL_VASE];

/// The plate key of spiral vase (Orca's `spiral_mode` in the 3MF).
pub const SPIRAL_VASE: &str = "spiral_vase";

/// Whether a plate option is offered on this printer: bed types and the plate's spiral vase
/// are Orca's plate settings.
pub fn offered(key: &str, orca: bool) -> bool {
    orca || !matches!(key, BED_TYPE | SPIRAL_VASE)
}

/// Plain label of a plate option.
pub fn label(key: &str) -> String {
    match key {
        BED_TYPE => "Bed type".into(),
        k => crate::settings::label(k),
    }
}

/// Takes the plate overrides out of a project plate's metadata (what stays is metadata Mica
/// keeps but does not apply: filament maps, filament sequences, thumbnails…).
pub fn take_overrides(metadata: &mut Settings) -> Settings {
    let mut out = Settings::new();
    if let Some(v) = metadata.remove("bed_type") {
        // Orca's legacy spelling.
        let v = if v == "SuperTack Plate" {
            "Supertack Plate".to_string()
        } else {
            v
        };
        out.insert(BED_TYPE.into(), v);
    }
    if let Some(v) = metadata.remove("print_sequence") {
        match v.trim() {
            "by object" => {
                out.insert("complete_objects".into(), "1".into());
            }
            "by layer" => {
                out.insert("complete_objects".into(), "0".into());
            }
            // Another sequence (Orca's "by default" is never written): keep it as it came.
            _ => {
                metadata.insert("print_sequence".into(), v);
            }
        }
    }
    if let Some(v) = metadata.remove("spiral_mode") {
        let on = matches!(v.trim(), "true" | "1");
        out.insert("spiral_vase".into(), if on { "1" } else { "0" }.into());
    }
    let mica: Vec<String> = metadata
        .keys()
        .filter(|k| k.starts_with(PREFIX))
        .cloned()
        .collect();
    for k in mica {
        if let Some(v) = metadata.remove(&k) {
            out.insert(k[PREFIX.len()..].to_string(), v);
        }
    }
    out
}

/// The plate overrides as project plate metadata (the inverse of [`take_overrides`]).
pub fn to_metadata(overrides: &Settings) -> Settings {
    let mut out = Settings::new();
    for (k, v) in overrides {
        match k.as_str() {
            BED_TYPE => {
                out.insert("bed_type".into(), v.clone());
            }
            "complete_objects" => {
                let on = matches!(v.trim(), "1" | "true");
                out.insert(
                    "print_sequence".into(),
                    if on { "by object" } else { "by layer" }.into(),
                );
            }
            "spiral_vase" => {
                let on = matches!(v.trim(), "1" | "true");
                out.insert("spiral_mode".into(), on.to_string());
            }
            _ => {
                out.insert(format!("{PREFIX}{k}"), v.clone());
            }
        }
    }
    out
}

/// The bed type a plate prints on, when it has its own.
pub fn bed_type(overrides: &Settings) -> Option<&str> {
    overrides.get(BED_TYPE).map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bed type and spiral vase are Orca's plate settings; print order is offered everywhere.
    #[test]
    fn orca_plate_options_are_offered_on_orca_printers() {
        assert!(offered(SPIRAL_VASE, true) && offered(BED_TYPE, true));
        assert!(!offered(SPIRAL_VASE, false) && !offered(BED_TYPE, false));
        assert!(offered("complete_objects", false));
        assert_eq!(label(SPIRAL_VASE), "Spiral vase");
    }

    #[test]
    fn orca_plate_keys_become_overrides_and_back() {
        let mut meta: Settings = [
            ("bed_type", "SuperTack Plate"),
            ("print_sequence", "by object"),
            ("spiral_mode", "true"),
            ("first_layer_print_sequence", "2 1"),
            ("mica_override:layer_height", "0.12"),
            ("filament_maps", "1 2"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let o = take_overrides(&mut meta);
        assert_eq!(o.get(BED_TYPE).map(String::as_str), Some("Supertack Plate"));
        assert_eq!(o.get("complete_objects").map(String::as_str), Some("1"));
        assert_eq!(o.get("spiral_vase").map(String::as_str), Some("1"));
        assert_eq!(o.get("layer_height").map(String::as_str), Some("0.12"));
        // What Mica does not apply stays metadata.
        assert_eq!(meta.len(), 2);
        assert!(meta.contains_key("first_layer_print_sequence"));
        let back = to_metadata(&o);
        assert_eq!(back["bed_type"], "Supertack Plate");
        assert_eq!(back["print_sequence"], "by object");
        assert_eq!(back["spiral_mode"], "true");
        assert_eq!(back["mica_override:layer_height"], "0.12");
        let mut again = back.clone();
        assert_eq!(take_overrides(&mut again), o);
        assert!(again.is_empty());
        let off: Settings = [("spiral_vase".to_string(), "0".to_string())].into();
        assert_eq!(to_metadata(&off)["spiral_mode"], "false");
    }
}
