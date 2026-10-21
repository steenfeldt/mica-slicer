// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

use mica_core::{
    config::Config,
    input::{self, PlateSelection},
    print_job::PreparedPrint,
    settings::PrintSettings,
    threemf::{self, Flavour},
};
use std::path::{Path, PathBuf};
fn gcode(path: &Path, plate: usize) -> Vec<u8> {
    let mut c = Config::defaults();
    for (k, v) in [
        ("use_relative_e_distances", "1"),
        ("gcode_flavor", "marlin2"),
        ("nozzle_diameter", "0.4,0.4,0.4,0.4"),
        ("filament_diameter", "1.75,1.75,1.75,1.75"),
        ("temperature", "210,210,210,210"),
        ("first_layer_temperature", "210,210,210,210"),
        ("extrusion_multiplier", "1,1,1,1"),
        ("wipe_tower", "0"),
        ("skirts", "0"),
        ("brim_width", "0"),
        ("perimeter_generator", "classic"),
        ("layer_height", "0.3"),
        ("first_layer_height", "0.3"),
    ] {
        c.set_raw(k, v).unwrap();
    }
    let loaded = input::load_with_plate(
        &[path.into()],
        (125., 110.),
        false,
        true,
        0,
        &Default::default(),
        &c,
        &mica_profiles::project::Mapper::default(),
        PlateSelection::Plate(plate),
    )
    .unwrap();
    let objects = input::prepare_objects(loaded.objects).unwrap();
    let job = PreparedPrint::new(objects, &c).unwrap();
    mica_gcode::export::export_gcode(
        &job,
        &c,
        &PrintSettings::from_config(&c).unwrap(),
        "roundtrip.3mf",
    )
    .unwrap()
    .gcode
    .into_bytes()
}
#[test]
fn both_flavours_produce_identical_mica_gcode() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/models");
    let tmp = std::env::temp_dir().join(format!("mica-export-gcode-{}.3mf", std::process::id()));
    struct Remove(PathBuf);
    impl Drop for Remove {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _remove = Remove(tmp.clone());
    for (name, plates) in [("painted-cube.3mf", 1), ("plates-orca.3mf", 2)] {
        let source = root.join(name);
        let project = threemf::load_3mf(&source).unwrap();
        let expected: Vec<_> = (1..=plates).map(|i| gcode(&source, i)).collect();
        for flavour in [Flavour::Orca, Flavour::Prusa] {
            threemf::write(&tmp, &project, flavour).unwrap();
            for (i, expected) in expected.iter().enumerate() {
                assert_eq!(
                    &gcode(&tmp, i + 1),
                    expected,
                    "{name} {flavour:?} plate {}",
                    i + 1
                );
            }
        }
    }
}
