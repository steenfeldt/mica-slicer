// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! First-run detection of the user's printers from an installed PrusaSlicer
//! (`PrusaSlicer.ini`: installed printer models and nozzles, last selected presets) and
//! Snapmaker Orca. The other slicers' own presets are imported on request only
//! (`mica_profiles::import`). Everything is read-only; nothing in those installations is
//! modified.

use mica_profiles::prusa::{Bundle, Kind};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PrusaSlicerState {
    /// Installed models from `[vendor:PrusaResearch]`: `(model id, nozzle variants)`.
    pub models: Vec<(String, Vec<String>)>,
    pub printer: String,
    pub print: String,
    pub filament: String,
}

/// Parses `PrusaSlicer.ini`. Only the sections Mica cares about are read.
pub fn parse_prusaslicer_ini(text: &str) -> PrusaSlicerState {
    let mut state = PrusaSlicerState::default();
    let mut section = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.to_string();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        match section.as_str() {
            "vendor:PrusaResearch" => {
                if let Some(model) = key.strip_prefix("model:") {
                    let variants = value
                        .split([';', ','])
                        .map(str::trim)
                        .filter(|v| !v.is_empty())
                        .map(str::to_string)
                        .collect();
                    state.models.push((model.to_string(), variants));
                }
            }
            "presets" => match key {
                "printer" => state.printer = value.to_string(),
                "print" => state.print = value.to_string(),
                "filament" => state.filament = value.to_string(),
                _ => {}
            },
            _ => {}
        }
    }
    state
}

/// Printer presets of `bundle` matching the installed models and nozzle variants, in bundle
/// order (sorted by name).
pub fn presets_for_models(bundle: &Bundle, models: &[(String, Vec<String>)]) -> Vec<String> {
    let mut out = Vec::new();
    for name in bundle.preset_names(Kind::Printer) {
        let Ok(flat) = bundle.flatten(Kind::Printer, name) else {
            continue;
        };
        let model = flat.get("printer_model").map(String::as_str).unwrap_or("");
        let variant = flat
            .get("printer_variant")
            .map(String::as_str)
            .unwrap_or("");
        if models
            .iter()
            .any(|(m, variants)| m == model && variants.iter().any(|v| v == variant))
        {
            out.push(name.to_string());
        }
    }
    out
}

/// PrusaSlicer's data directory, if installed.
pub fn prusaslicer_dir() -> Option<PathBuf> {
    let home = mica_core::user_data::home()?;
    let candidates = [
        home.join("Library/Application Support/PrusaSlicer"),
        home.join(".config/PrusaSlicer"),
        home.join(".var/app/com.prusa3d.PrusaSlicer/config/PrusaSlicer"),
    ];
    let mut candidates = candidates.to_vec();
    if let Some(appdata) = mica_core::user_data::var_dir("APPDATA") {
        candidates.push(Path::new(&appdata).join("PrusaSlicer"));
    }
    candidates
        .into_iter()
        .find(|p| p.join("PrusaSlicer.ini").is_file())
}

/// Whether Snapmaker Orca (the U1's slicer) has ever been run here.
pub fn snapmaker_orca_installed() -> bool {
    mica_core::user_data::home().is_some_and(|home| {
        home.join("Library/Application Support/Snapmaker_Orca")
            .is_dir()
            || home.join(".config/Snapmaker_Orca").is_dir()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests never read the user's PrusaSlicer or OrcaSlicer settings (their last filament,
    /// user presets) or Mica's own settings folder (`mica_core::user_data`): an editor test
    /// once imported the real last PrusaSlicer filament, which the CLI it compared with (on
    /// an empty settings folder) did not know.
    #[test]
    fn tests_never_read_the_users_slicer_settings() {
        assert!(mica_core::user_data::isolated());
        assert_eq!(prusaslicer_dir(), None);
        assert!(!snapmaker_orca_installed());
        if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
            let real = PathBuf::from(home).join("Library/Application Support/Mica Slicer");
            assert!(!crate::state::Prefs::default_path().starts_with(&real));
            assert!(!mica_profiles::user::Store::default_root().starts_with(&real));
            assert!(!mica_library::default_data_dir().starts_with(&real));
        }
    }

    const INI: &str = "\
# generated by PrusaSlicer 2.9.6
version = 2.9.6

[presets]
filament = Extrudr GreenTEC Pro @COREONE HF0.4 Tweaked
physical_printer =
print = 0.20mm SPEED @COREONE HF0.4
printer = Prusa CORE One HF0.4 nozzle

[vendor:PrusaResearch]
model:COREONE = HF0.4
model:XL5IS = 0.4
model:MK4S = 0.4; 0.6
";

    const BUNDLE: &str = "\
[vendor]
name = PrusaResearch
[printer:*common*]
nozzle_diameter = 0.4
[printer:Prusa CORE One HF0.4 nozzle]
inherits = *common*
printer_model = COREONE
printer_variant = HF0.4
[printer:Prusa CORE One 0.4 nozzle]
inherits = *common*
printer_model = COREONE
printer_variant = 0.4
[printer:Original Prusa XL 5T Input Shaper 0.4 nozzle]
printer_model = XL5IS
printer_variant = 0.4
[printer:Original Prusa MINI & MINI+]
printer_model = MINI
printer_variant = 0.4
[printer:Original Prusa MK4S 0.6 nozzle]
printer_model = MK4S
printer_variant = 0.6
";

    #[test]
    fn detects_installed_models_and_last_presets() {
        let s = parse_prusaslicer_ini(INI);
        assert_eq!(s.printer, "Prusa CORE One HF0.4 nozzle");
        assert_eq!(s.print, "0.20mm SPEED @COREONE HF0.4");
        assert_eq!(s.filament, "Extrudr GreenTEC Pro @COREONE HF0.4 Tweaked");
        assert_eq!(
            s.models,
            vec![
                ("COREONE".to_string(), vec!["HF0.4".to_string()]),
                ("XL5IS".to_string(), vec!["0.4".to_string()]),
                (
                    "MK4S".to_string(),
                    vec!["0.4".to_string(), "0.6".to_string()]
                ),
            ]
        );
        let bundle = Bundle::parse(BUNDLE);
        assert_eq!(
            presets_for_models(&bundle, &s.models),
            vec![
                "Original Prusa MK4S 0.6 nozzle",
                "Original Prusa XL 5T Input Shaper 0.4 nozzle",
                "Prusa CORE One HF0.4 nozzle",
            ]
        );
    }

    #[test]
    fn missing_sections_are_fine() {
        let s = parse_prusaslicer_ini("[recent]\nskein_directory = /x\n");
        assert_eq!(s, PrusaSlicerState::default());
    }
}
