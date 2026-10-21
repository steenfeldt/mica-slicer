// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The export's printable-area check against the real bed (`mica_core::build_volume`), as
//! PrusaSlicer and Orca test sliced toolpaths (`BuildVolume::all_paths_inside`): extrusion
//! centerlines on the bed outline (rectangle, a delta's circle, a polygon), and under Orca
//! rules outside the bed's exclusion areas. A print outside is refused with the object, the
//! feature and where; nothing is written.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn model(name: &str) -> String {
    format!("{}/../../tests/models/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn out() -> PathBuf {
    std::env::temp_dir().join(format!(
        "mica-build-volume-{}-{}.gcode",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

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
/// A delta: `printable_area` is a 72-point circle of radius 130 around the origin.
const FLSUN_T1: &[&str] = &["--vendor", "FLSun", "--printer", "FLSun T1 0.4 nozzle"];
/// A concave bed: two tabs reach past the back edge (Y 265 to 273) at X 37.7–75.5 and
/// 189.7–227.5; between them the bed ends at Y 265.
const ARTILLERY_M1_PRO: &[&str] = &[
    "--vendor",
    "Artillery",
    "--printer",
    "Artillery M1 Pro 0.6 nozzle",
];
const DIAMOND: &str = "bed_shape=125x0,250x125,125x250,0x125";

enum Sliced {
    Ok,
    Refused(String),
    Skipped,
}

/// `mica slice cube20.stl <profile> --center <center> <extra>`.
fn slice(profile: &[&str], center: &str, extra: &[&str]) -> Sliced {
    let path = out();
    let mut all: Vec<String> = vec!["slice".into(), model("cube20.stl")];
    all.extend(profile.iter().map(|s| s.to_string()));
    all.extend(["--center".into(), center.into()]);
    all.extend(extra.iter().map(|s| s.to_string()));
    all.extend(["-o".into(), path.to_string_lossy().into_owned()]);
    let result = mica_cli::run(&all);
    let written = path.exists();
    let _ = std::fs::remove_file(&path);
    match result {
        Ok(()) => {
            assert!(written, "{all:?}: no G-code written");
            Sliced::Ok
        }
        Err(e) if e.contains("no Orca") || e.contains("no Prusa vendor profiles") => {
            eprintln!("skip: {e}");
            Sliced::Skipped
        }
        Err(e) => {
            assert!(!written, "{all:?}: refused but left {}", path.display());
            Sliced::Refused(e)
        }
    }
}

fn prints(profile: &[&str], center: &str, extra: &[&str]) {
    match slice(profile, center, extra) {
        Sliced::Refused(e) => panic!("{center} {extra:?} refused: {e}"),
        Sliced::Ok | Sliced::Skipped => {}
    }
}

/// The refusal's message, `None` when skipped.
fn refused(profile: &[&str], center: &str, extra: &[&str]) -> Option<String> {
    match slice(profile, center, extra) {
        Sliced::Ok => panic!("{center} {extra:?} printed"),
        Sliced::Refused(e) => Some(e),
        Sliced::Skipped => None,
    }
}

#[test]
fn diamond_bed_refuses_objects_and_brims_off_its_outline() {
    // The cube (X/Y 20–40) is inside the bed's bounding box, entirely off the diamond.
    let e = refused(&[], "30,30", &["--set", DIAMOND]).unwrap();
    assert!(
        e.contains("cube20.stl") && e.contains("outside the printer's build volume"),
        "{e}"
    );
    prints(&[], "125,125", &["--set", DIAMOND]);
    // The cube is on the bed, its 10 mm brim crosses the lower-left edge.
    prints(&[], "65,82", &["--set", DIAMOND]);
    let e = refused(&[], "65,82", &["--set", DIAMOND, "--set", "brim_width=10"]).unwrap();
    assert!(
        e.contains("the brim of cube20.stl is outside the printable volume")
            && e.contains("outside the bed outline"),
        "{e}"
    );
}

#[test]
fn delta_bed_is_a_circle() {
    prints(FLSUN_T1, "0,0", &[]);
    prints(FLSUN_T1, "0,-95", &[]);
    // Corners at (95, 95): 134 mm from the center, past the 130 mm radius, though inside the
    // bed's bounding box.
    let Some(e) = refused(FLSUN_T1, "85,85", &[]) else {
        return;
    };
    assert!(e.contains("outside the printer's build volume"), "{e}");
    // The cube fits (its corner 124.5 mm out), a skirt 6 mm around it does not.
    let skirt = ["--set", "skirt_loops=1", "--set", "skirt_distance=6"];
    prints(FLSUN_T1, "70,70", &skirt);
    let e = refused(FLSUN_T1, "78,78", &skirt).unwrap();
    assert!(
        e.contains("the skirt is outside the printable volume")
            && e.contains("outside the round bed"),
        "{e}"
    );
}

#[test]
fn concave_bed_uses_its_real_outline() {
    // In the left tab, which reaches past the back edge.
    prints(ARTILLERY_M1_PRO, "56,262", &[]);
    // Between the tabs: inside the convex hull, past the bed's back edge.
    let Some(e) = refused(ARTILLERY_M1_PRO, "130,262", &[]) else {
        return;
    };
    assert!(e.contains("outside the printer's build volume"), "{e}");
}

#[test]
fn orca_exclusion_areas_refuse_objects_and_toolpaths() {
    let zone = "bed_exclude_area=100x100,170x100,170x170,100x170";
    prints(U1, "135,135", &[]);
    let Some(e) = refused(U1, "135,135", &["--set", zone]) else {
        return;
    };
    assert!(
        e.contains("cube20.stl: model reaches into the excluded area X 100–170, Y 100–170 mm"),
        "{e}"
    );
    // The cube (X 75–95) stays out of the area; its 12 mm brim reaches in.
    prints(U1, "85,135", &["--set", zone]);
    let e = refused(
        U1,
        "85,135",
        &[
            "--set",
            zone,
            "--set",
            "brim_width=12",
            "--set",
            "brim_type=outer_only",
        ],
    )
    .unwrap();
    assert!(
        e.contains("the brim of cube20.stl is outside the printable volume")
            && e.contains("in the excluded area X 100–170, Y 100–170 mm"),
        "{e}"
    );
    // Bambu's cutter area in the front-left corner.
    let cutter = "bed_exclude_area=0x0,18x0,18x28,0x28";
    let e = refused(U1, "20,25", &["--set", cutter]).unwrap();
    assert!(e.contains("excluded area X 0–18, Y 0–28 mm"), "{e}");
    prints(U1, "30,25", &["--set", cutter]);
}

#[test]
fn prusa_rules_test_centerlines_at_the_bed_edge() {
    // CORE One (bed X 0–250), cube20 with a 5 mm brim: its outermost loop's centerline is
    // on the bed down to a cube center of X 14.3, as PrusaSlicer's toolpath test accepts.
    // The old half-width margin refused it from X 14.45.
    prints(&[], "14.3,110", &["--set", "brim_width=5"]);
    let e = refused(&[], "14.0,110", &["--set", "brim_width=5"]).unwrap();
    assert!(
        e.contains("The brim of cube20.stl goes") && e.contains("past the left edge of the bed"),
        "{e}"
    );
}
