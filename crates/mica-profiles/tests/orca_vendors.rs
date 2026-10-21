// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Every printer vendor of the pinned OrcaSlicer profiles: the gallery's machine-only load and
//! (ignored, a report) the full bundles with each printer's default process and filament.

use mica_profiles::orca::{Bundle, Dialect, Kind, discovery};
use std::path::PathBuf;
use std::time::Instant;

fn root() -> Option<PathBuf> {
    let root = mica_testkit::orca_profiles();
    mica_testkit::require_fixture(
        root.join("Ratrig.json"),
        "OrcaSlicer profiles: scripts/fetch-upstream.sh --profiles orca",
    )
    .then_some(root)
}

#[test]
fn every_vendor_lists_its_printers_without_process_or_filament_files() {
    let Some(root) = root() else { return };
    let start = Instant::now();
    let vendors = discovery::vendor_indexes(&root);
    for v in [
        "Ratrig",
        "Voron",
        "Creality",
        "BBL",
        "Snapmaker",
        "Co Print",
    ] {
        assert!(vendors.iter().any(|x| x == v), "{v} missing: {vendors:?}");
    }
    assert!(!vendors.iter().any(|v| v == "OrcaFilamentLibrary"));
    let mut machines = 0;
    for v in &vendors {
        let b = Bundle::load_vendor_machines(root.join(format!("{v}.json")), Dialect::Orca242)
            .unwrap_or_else(|e| panic!("{v}: {e}"));
        assert!(b.preset_names(Kind::Process).is_empty());
        assert_eq!(b.primary_vendor(), v);
        for name in b.preset_names(Kind::Machine) {
            let flat = b
                .flatten(Kind::Machine, name)
                .unwrap_or_else(|e| panic!("{v} / {name}: {e}"));
            assert!(flat.contains_key("printable_area"), "{v} / {name}");
            machines += 1;
        }
    }
    eprintln!(
        "{} vendors, {machines} printers in {:?}",
        vendors.len(),
        start.elapsed()
    );
    assert!(machines > 900, "{machines}");
    // Vendor index names with spaces are names, not paths.
    assert!(discovery::is_index_name("Wanhao France"));
    assert!(!discovery::is_index_name("../Ratrig"));
    assert!(!discovery::is_index_name(" Ratrig"));
}

/// `cargo test -p mica-profiles --test orca_vendors -- --ignored --nocapture`: loads every
/// vendor's full bundle and composes each printer with a compatible 0.20 mm process and a PLA,
/// printing what fails per vendor.
#[test]
#[ignore]
fn report_full_bundles_per_vendor() {
    let Some(root) = root() else { return };
    for v in discovery::vendor_indexes(&root) {
        let start = Instant::now();
        let (b, _) = match discovery::load_installed(Some(&root), &v) {
            Ok(b) => b,
            Err(e) => {
                println!("{v}: LOAD FAILED {e}");
                continue;
            }
        };
        let load = start.elapsed();
        let printers = b.preset_names(Kind::Machine);
        let mut ok = 0;
        let mut problems: Vec<String> = vec![];
        for p in &printers {
            let processes = b.compatible_presets(Kind::Process, p, None);
            let Some(process) = processes
                .iter()
                .find(|n| n.contains("0.20"))
                .or(processes.first())
            else {
                problems.push(format!("{p}: no process"));
                continue;
            };
            let filaments = b.compatible_presets(Kind::Filament, p, Some(process));
            let Some(filament) = filaments
                .iter()
                .find(|n| n.contains("PLA"))
                .or(filaments.first())
            else {
                problems.push(format!("{p}: no filament"));
                continue;
            };
            match b.single_tool_config(p, process, filament, None) {
                Ok(_) => ok += 1,
                Err(e) => problems.push(format!("{p}: {}", e.0.lines().next().unwrap_or(""))),
            }
        }
        println!(
            "{v}: {} printers, {ok} compose, load {load:?}, total {:?}",
            printers.len(),
            start.elapsed()
        );
        problems.sort();
        problems.dedup();
        for p in problems.iter().take(6) {
            println!("    {p}");
        }
        if problems.len() > 6 {
            println!("    … {} more", problems.len() - 6);
        }
    }
}
