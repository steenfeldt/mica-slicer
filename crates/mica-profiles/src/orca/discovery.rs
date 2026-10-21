// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/OrcaSlicer.cpp :: CLI::setup
// Source: orca@8500fcdcca CMakeLists.txt :: SLIC3R_FHS_RESOURCES
// Source: orca@8500fcdcca scripts/flatpak/com.orcaslicer.OrcaSlicer.yml :: app-id
// Changes: Read-only cross-platform profile discovery, explicit override, checkout fallback.
// Changes: `Orca:` vendor prefix selects Orca's index of a vendor name PrusaSlicer also uses.
// Changes: Snapmaker Orca resources (flow-variant presets) load with the Snapmaker dialect.
// Changes: vendor_indexes lists every vendor of a profiles folder; index names may hold spaces.
// Changes: The home folder comes from mica_core::user_data (none in tests and test tools).
// Changes: preset_vendors maps preset names to vendor indexes for importing user presets (original).
// Changes: Checkout fallback via mica_core::user_data::upstream_dir (not packaged; first in tests).
// Changes: Checkout first, then the subset a packaged Mica ships, then installed OrcaSlicer.

use std::path::{Path, PathBuf};

use super::{Bundle, Dialect, Error, Kind};

/// `Orca:Prusa` names OrcaSlicer's own vendor where Mica's plain name means another source
/// (plain `Prusa` is PrusaSlicer's bundle).
pub fn vendor_index(vendor: &str) -> &str {
    vendor.strip_prefix("Orca:").unwrap_or(vendor)
}

/// A vendor index name as Orca's resources spell them (`Snapmaker`, `Co Print`,
/// `Wanhao France`): letters, digits, `_`, `-` and inner spaces, never a path.
pub fn is_index_name(vendor: &str) -> bool {
    !vendor.is_empty()
        && vendor.trim() == vendor
        && vendor
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == ' ')
}

/// Index files that are not printer vendors (the shared filament library, Orca's lists).
const NOT_VENDORS: [&str; 2] = ["OrcaFilamentLibrary", "blacklist"];

/// Every printer vendor index in a profiles folder (`<Vendor>.json` next to a `<Vendor>/`
/// folder), sorted by name. Reads only the directory listing.
pub fn vendor_indexes(root: &Path) -> Vec<String> {
    let Ok(dir) = std::fs::read_dir(root) else {
        return vec![];
    };
    let mut out: Vec<String> = dir
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            let stem = path.file_stem()?.to_str()?.to_string();
            (path.extension().is_some_and(|x| x == "json")
                && is_index_name(&stem)
                && !NOT_VENDORS.contains(&stem.as_str())
                && root.join(&stem).is_dir())
            .then_some(stem)
        })
        .collect();
    out.sort();
    out
}

/// Which vendor index lists a preset: `(kind, preset name)` → index names (several when
/// vendors share a name; `OrcaFilamentLibrary` for the shared filaments). Reads only the index
/// files' `machine_list` / `process_list` / `filament_list`, so finding the vendor an imported
/// user preset builds on does not load every vendor's presets.
pub fn preset_vendors(root: &Path) -> std::collections::BTreeMap<(Kind, String), Vec<String>> {
    let mut out: std::collections::BTreeMap<(Kind, String), Vec<String>> = Default::default();
    let mut indexes = vendor_indexes(root);
    indexes.push("OrcaFilamentLibrary".into());
    for vendor in indexes {
        let Ok(text) = std::fs::read_to_string(root.join(format!("{vendor}.json"))) else {
            continue;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        for kind in [Kind::Machine, Kind::Process, Kind::Filament] {
            let list = json
                .get(format!("{}_list", kind.directory()))
                .and_then(|l| l.as_array());
            for entry in list.into_iter().flatten() {
                if let Some(name) = entry.get("name").and_then(|n| n.as_str()) {
                    out.entry((kind, name.to_string()))
                        .or_default()
                        .push(vendor.clone());
                }
            }
        }
    }
    out
}

/// A development checkout's pinned copy (never in a packaged build), then the profiles shipped
/// with a packaged Mica (`vendor_profiles::bundled_orca`), then an installed OrcaSlicer.
/// Portable installs can use an explicit root.
pub fn find_profiles(explicit: Option<&Path>, vendor: &str) -> Result<PathBuf, Error> {
    let vendor = vendor_index(vendor);
    if !is_index_name(vendor) {
        return Err(Error("vendor must be an index name, e.g. Snapmaker".into()));
    }
    let index = format!("{vendor}.json");
    if let Some(path) = explicit {
        return if path.join(&index).is_file() {
            Ok(path.into())
        } else {
            Err(Error(format!("{} has no {index}", path.display())))
        };
    }
    let mut paths = vec![];
    // A development checkout's pinned copy (never in a packaged build): tests, test tools and
    // `cargo run` slice with the pinned profiles whatever OrcaSlicer happens to be installed.
    if let Some(upstream) = mica_core::user_data::upstream_dir() {
        paths.push(upstream.join("orca/resources/profiles"));
    }
    // The subset a packaged Mica ships (the same pinned OrcaSlicer profiles).
    paths.extend(crate::vendor_profiles::bundled_orca());
    paths.push(PathBuf::from(
        "/Applications/OrcaSlicer.app/Contents/Resources/profiles",
    ));
    for prefix in [
        "/usr/local/share/OrcaSlicer",
        "/usr/share/OrcaSlicer",
        "/app/share/OrcaSlicer",
        "/app/resources",
        "/opt/OrcaSlicer/resources",
    ] {
        paths.push(Path::new(prefix).join("profiles"));
    }
    if let Some(appdir) = std::env::var_os("APPDIR") {
        paths.push(Path::new(&appdir).join("resources/profiles"));
    }
    for env in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
        if let Some(prefix) = std::env::var_os(env) {
            let prefix = PathBuf::from(prefix);
            paths.push(prefix.join("OrcaSlicer/resources/profiles"));
            paths.push(prefix.join("Programs/OrcaSlicer/resources/profiles"));
        }
    }
    // Locate portable Windows/Linux installations through their executable on PATH.
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            for exe in ["orca-slicer", "OrcaSlicer", "orca-slicer.exe"] {
                if let Ok(exe) = dir.join(exe).canonicalize() {
                    let bin = exe.parent().unwrap();
                    paths.push(bin.join("resources/profiles"));
                    if let Some(parent) = bin.parent() {
                        paths.push(parent.join("resources/profiles"));
                    }
                }
            }
        }
    }
    let mut flatpak_roots = vec![PathBuf::from("/var/lib/flatpak")];
    if let Some(home) = mica_core::user_data::home() {
        flatpak_roots.push(Path::new(&home).join(".local/share/flatpak"));
    }
    for root in flatpak_roots {
        for id in [
            "com.orcaslicer.OrcaSlicer",
            "io.github.softfever.OrcaSlicer",
        ] {
            for arch in ["x86_64", "aarch64"] {
                let files = root.join(format!("app/{id}/{arch}/stable/active/files"));
                paths.push(files.join("share/OrcaSlicer/profiles"));
                paths.push(files.join("resources/profiles"));
            }
        }
    }
    paths
        .into_iter()
        .find(|p| p.join(&index).is_file())
        .ok_or_else(|| {
            Error(format!(
                "no Orca {vendor} profiles found: Mica ships only some vendors; install OrcaSlicer or pass --profiles-dir"
            ))
        })
}

pub fn load_installed(explicit: Option<&Path>, vendor: &str) -> Result<(Bundle, PathBuf), Error> {
    let vendor = vendor_index(vendor);
    let root = find_profiles(explicit, vendor)?;
    let mut bundle = Bundle::new(Dialect::Orca242);
    let library = root.join("OrcaFilamentLibrary.json");
    if vendor != "OrcaFilamentLibrary" && library.is_file() {
        bundle.add_vendor(library)?;
    }
    bundle.add_vendor(root.join(format!("{vendor}.json")))?;
    // Snapmaker Orca's own resources (its app or the pinned fork) use the flow-variant format.
    if bundle.uses_flow_variants() {
        bundle.dialect = Dialect::Snapmaker240;
    }
    Ok((bundle, root))
}
