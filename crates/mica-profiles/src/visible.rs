// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/Preset.cpp :: Preset::set_visible_from_appconfig
// Source: orca@8500fcdcca src/libslic3r/AppConfig.cpp :: AppConfig::load, AppConfig::save
// Source: prusaslicer@b028299c77 src/libslic3r/Preset.cpp :: Preset::set_visible_from_appconfig
// Changes: Read-only: the other slicers' visible-filament lists only seed Mica's own list per
// Changes: printer (mica.toml); a list naming no filament counts as nothing to seed.

//! Which filaments the user chose to see in OrcaSlicer, Snapmaker Orca and PrusaSlicer, so
//! Mica's filament picker can start from the same list. Both slicers keep one list for all
//! printers in their app settings: OrcaSlicer's `<app>.conf` (JSON, `"filaments": [names]`),
//! PrusaSlicer's `PrusaSlicer.ini` (`[filaments]`, `name = 1`). Only system filaments are
//! subject to it; a filament renamed since counts under its old names too. Nothing here
//! writes to those files.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::import::{Source, app_bases};

/// The filament names in an OrcaSlicer / Snapmaker Orca app config (`OrcaSlicer.conf`), or
/// `None` when it has no list. The trailing `# MD5 checksum` line Orca writes is ignored.
pub fn orca_selected(conf: &str) -> Option<BTreeSet<String>> {
    let json: String = conf
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    let value: serde_json::Value = serde_json::from_str(&json).ok()?;
    let list = value.get("filaments")?.as_array()?;
    let names: BTreeSet<String> = list
        .iter()
        .filter_map(|v| v.as_str())
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .collect();
    (!names.is_empty()).then_some(names)
}

/// The installed filaments in `PrusaSlicer.ini` (`[filaments]`, a non-empty value), or `None`
/// when it has none.
pub fn prusa_installed(ini: &str) -> Option<BTreeSet<String>> {
    let names: BTreeSet<String> = crate::ini::parse(ini)
        .into_iter()
        .filter(|s| s.name == "filaments")
        .flat_map(|s| s.entries)
        .filter(|(_, v)| !v.trim().is_empty())
        .map(|(k, _)| k.trim().to_string())
        .collect();
    (!names.is_empty()).then_some(names)
}

/// The app settings file of `source` for the user whose home folder is `home`, if it exists.
pub fn settings_file(source: Source, home: &Path, appdata: Option<&Path>) -> Option<PathBuf> {
    let file = match source {
        Source::PrusaSlicer => "PrusaSlicer.ini",
        Source::OrcaSlicer => "OrcaSlicer.conf",
        Source::SnapmakerOrca => "Snapmaker_Orca.conf",
    };
    app_bases(source, home, appdata)
        .into_iter()
        .map(|b| b.join(file))
        .find(|f| f.is_file())
}

/// The visible-filament list `source` keeps, read from its settings file.
pub fn read(source: Source, home: &Path, appdata: Option<&Path>) -> Option<BTreeSet<String>> {
    let text = std::fs::read_to_string(settings_file(source, home, appdata)?).ok()?;
    if source.is_orca() {
        orca_selected(&text)
    } else {
        prusa_installed(&text)
    }
}

/// Whether a system filament named `name` (once called `renamed_from`) is on the list.
pub fn listed(list: &BTreeSet<String>, name: &str, renamed_from: &[String]) -> bool {
    list.contains(name) || renamed_from.iter().any(|n| list.contains(n))
}

/// A preset's `renamed_from` value as names: PrusaSlicer writes `"A";"B"`, Orca one name.
pub fn renamed_from(raw: &str) -> Vec<String> {
    raw.split(';')
        .map(|n| n.trim().trim_matches('"').trim())
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_both_formats_from_a_fake_home() {
        let home = std::env::temp_dir().join(format!("mica-visible-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let orca = home.join("Library/Application Support/OrcaSlicer");
        std::fs::create_dir_all(&orca).unwrap();
        std::fs::write(
            orca.join("OrcaSlicer.conf"),
            "{\n  \"app\": {\"language\": \"en\"},\n  \"filaments\": [\"Generic PLA @System\", \"Snapmaker PLA @U1\", \"\"]\n}\n# MD5 checksum 0123456789ABCDEF0123456789ABCDEF\n",
        )
        .unwrap();
        let prusa = home.join(".config/PrusaSlicer");
        std::fs::create_dir_all(&prusa).unwrap();
        std::fs::write(
            prusa.join("PrusaSlicer.ini"),
            "[filaments]\nPrusament PLA @COREONE HF0.4 = 1\nPrusament PETG = \n\n[presets]\nfilament = X\n",
        )
        .unwrap();
        let orca_list = read(Source::OrcaSlicer, &home, None).unwrap();
        assert_eq!(
            orca_list.into_iter().collect::<Vec<_>>(),
            ["Generic PLA @System", "Snapmaker PLA @U1"]
        );
        assert_eq!(read(Source::SnapmakerOrca, &home, None), None);
        let prusa_list = read(Source::PrusaSlicer, &home, None).unwrap();
        assert_eq!(
            prusa_list.iter().collect::<Vec<_>>(),
            ["Prusament PLA @COREONE HF0.4"]
        );
        assert!(listed(
            &prusa_list,
            "Prusament PLA @CORE One HF0.4",
            &renamed_from("\"Prusament PLA @COREONE HF0.4\"; \"Old\"")
        ));
        assert!(!listed(&prusa_list, "Prusament PETG", &[]));
        // No list, or an empty one: nothing to seed.
        assert_eq!(orca_selected("{\"app\": {}}"), None);
        assert_eq!(prusa_installed("[filaments]\n"), None);
    }
}
