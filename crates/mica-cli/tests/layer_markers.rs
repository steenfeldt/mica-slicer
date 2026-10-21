// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! GUI monkey test, seed 3007: two objects with different layer heights printed layers at
//! Z 0.799707 and Z 0.8. Both layer markers read `;Z:.8` (three decimals), so the second
//! layer seemed not to rise after the first one's extrusions at Z 0.8. PrusaSlicer writes
//! `;Z:` with six significant digits (`float_to_string_decimal_point`), OrcaSlicer with `%g`.
use mica_core::threemf::{self, Flavour};
use std::path::{Path, PathBuf};

const U1: &[&str] = &[
    "--vendor",
    "Snapmaker",
    "--printer",
    "Snapmaker U1 (0.4 nozzle)",
    "--print",
    "0.20 Standard @Snapmaker U1 (0.4 nozzle)",
    "--filament",
    "Snapmaker PLA @U1",
    "--bed",
    "High Temp Plate",
];

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn layers_a_fraction_of_a_micron_apart_keep_distinct_markers() {
    if !mica_testkit::require_orca_profiles() {
        return;
    }
    let dir: PathBuf =
        std::env::temp_dir().join(format!("mica-layer-markers-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // plates-orca.3mf, plate 1: two objects; the first one at 0.2999 mm layers, so its
    // layers come 0.2 µm under the other one's (0.2 mm layers): Z 0.8498 and 0.85.
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/models/plates-orca.3mf");
    let mut project = threemf::load_3mf(&source).unwrap();
    project.objects[0]
        .settings
        .insert("layer_height".into(), "0.2999".into());
    let (input, out) = (dir.join("in.3mf"), dir.join("out.gcode"));
    threemf::write(&input, &project, Flavour::Orca).unwrap();
    let path = |p: &Path| p.to_string_lossy().into_owned();
    let mut all = args(&[
        "slice",
        &path(&input),
        "--plate",
        "1",
        "--set",
        "enable_prime_tower=0",
    ]);
    all.extend(args(U1));
    all.extend(["-o".into(), path(&out)]);
    mica_cli::run(&all).unwrap_or_else(|e| panic!("mica {all:?}: {e}"));
    let gcode = std::fs::read_to_string(&out).unwrap();
    let markers: Vec<&str> = gcode
        .lines()
        .filter_map(|l| l.strip_prefix(";Z:"))
        .collect();
    assert!(markers.contains(&"0.8498"), "{markers:?}");
    assert!(markers.contains(&"0.85"), "{markers:?}");
    let mut validate = args(&["validate", &path(&out)]);
    validate.extend(args(U1));
    mica_cli::run(&validate).unwrap_or_else(|e| panic!("validate: {e}"));
    let _ = std::fs::remove_dir_all(&dir);
}
