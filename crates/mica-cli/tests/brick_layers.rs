// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Brick layers end to end: G-code sliced with brick layers obeys the product rules of
//! `mica_core::brick_layers` (checked by `mica_gcode::brick_check` against the same print
//! without brick layers), and the feature is a hidden opt-in (`brick_layers = true` under
//! `[features]` in mica.toml).
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mica-bricks-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn settings(dir: &Path, unlocked: bool) -> PathBuf {
    let settings = dir.join(if unlocked { "unlocked" } else { "plain" });
    std::fs::create_dir_all(&settings).unwrap();
    if unlocked {
        std::fs::write(
            settings.join("mica.toml"),
            "[features]\nbrick_layers = true\n",
        )
        .unwrap();
    }
    settings
}

fn model(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/models")
        .join(name)
        .to_str()
        .unwrap()
        .to_string()
}

/// Slices `args` (after the model) into G-code text; returns it with the stderr text.
fn slice(settings: &Path, dir: &Path, out: &str, model: &str, args: &[&str]) -> (String, String) {
    let out = dir.join(out);
    let r = Command::new(env!("CARGO_BIN_EXE_mica"))
        .env("MICA_CONFIG_DIR", settings)
        .env("SOURCE_DATE_EPOCH", "0")
        .arg("slice")
        .arg(model)
        .args(args)
        .arg("-o")
        .arg(&out)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&r.stderr).into_owned();
    assert!(r.status.success(), "{stderr}");
    (std::fs::read_to_string(&out).unwrap(), stderr)
}

const U1: [&str; 10] = [
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

/// Slices `model` with and without brick layers and checks the rules; returns the number of
/// raised extrusions.
fn checked(name: &str, model: &str, args: &[&str], shells: (usize, usize)) -> usize {
    checked_plate(name, model, args, shells, true)
}

fn checked_plate(
    name: &str,
    model: &str,
    args: &[&str],
    shells: (usize, usize),
    one_object: bool,
) -> usize {
    let dir = temp(name);
    let unlocked = settings(&dir, true);
    let (off, _) = slice(&unlocked, &dir, "off.gcode", model, args);
    let mut on_args = args.to_vec();
    on_args.extend(["--set", "mica_brick_layers=1"]);
    let (on, said) = slice(&unlocked, &dir, "on.gcode", model, &on_args);
    assert!(!said.contains("which are off"), "{said}");
    let report = mica_gcode::brick_check::check_with(&on, &off, shells.0, shells.1, one_object);
    assert!(
        report.problems.is_empty(),
        "{name}: {:#?}\nraised {}, heights {}..{}, flow {}",
        &report.problems[..report.problems.len().min(12)],
        report.raised,
        report.min_relative_height,
        report.max_relative_height,
        report.min_flow_ratio
    );
    assert!(report.raised > 0, "{name}: nothing was raised");
    // Every bead is at least a layer tall (the G-code's Z has 1 µm steps).
    assert!(
        report.min_relative_height > 1.0 - 2e-2,
        "{name}: {report:?}"
    );
    assert!(report.min_flow_ratio >= 1.0 - 5e-3, "{name}: {report:?}");
    let _ = std::fs::remove_dir_all(&dir);
    report.raised
}

#[test]
fn core_one_cube() {
    // CORE One 0.2 mm profile: 2 bottom, 3 top shell layers.
    checked("core-one-cube", &model("cube20.stl"), &[], (4, 6));
}

#[test]
fn core_one_cube_arachne_four_walls() {
    checked(
        "core-one-arachne",
        &model("cube20.stl"),
        &["--perimeter-generator", "arachne", "--set", "perimeters=4"],
        (4, 6),
    );
}

#[test]
fn u1_cube() {
    checked("u1-cube", &model("cube20.stl"), &U1, (4, 6));
}

#[test]
fn mid_part_tops_steps_and_cone() {
    // The stepped pyramid has a top surface on every step; the cone's wall slopes.
    checked(
        "shells",
        &model("shells.stl"),
        &["--set", "perimeters=3"],
        (4, 6),
    );
}

#[test]
fn hole_next_to_contour() {
    checked(
        "hole-block",
        &model("hole_block.stl"),
        &["--set", "perimeters=4"],
        (4, 6),
    );
}

#[test]
fn variable_layer_height() {
    // Adaptive layer heights travel in a project file.
    let dir = temp("adaptive-project");
    let project = dir.join("adaptive.3mf");
    let r = Command::new(env!("CARGO_BIN_EXE_mica"))
        .env("MICA_CONFIG_DIR", settings(&dir, true))
        .env("SOURCE_DATE_EPOCH", "0")
        .args(["slice", &model("shells.stl"), "--adaptive-layers", "0.5"])
        .arg("--export-3mf")
        .arg(&project)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    checked("adaptive", project.to_str().unwrap(), &[], (4, 6));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn u1_steps_and_cone() {
    // Orca rules and Klipper output on the stepped pyramid and cone (Benchy runs in the
    // brick-layers validation, docs/brick-layers.md: too slow for an unoptimized test build).
    checked("shells-u1", &model("shells.stl"), &U1, (4, 6));
}

#[test]
fn two_objects_on_one_plate() {
    // Through the plate scheduler; the second object is the stepped pyramid and cone.
    checked_plate(
        "plate",
        &model("cube20.stl"),
        &[&model("shells.stl")],
        (4, 6),
        false,
    );
}

#[test]
fn several_tools_slice_without_brick_layers() {
    let dir = temp("tools");
    let unlocked = settings(&dir, true);
    let xl = [
        "--printer",
        "Original Prusa XL - 5T Input Shaper 0.4 nozzle",
        "--print",
        "0.20mm SPEED @XLIS 0.4",
        "--filament",
        "Prusament PLA @XLIS",
    ];
    let cube = model("cube20.stl");
    let shells = model("shells.stl");
    let mut args = vec![shells.as_str()];
    args.extend(xl);
    args.extend(["--object-extruder", "2=2"]);
    let (off, _) = slice(&unlocked, &dir, "off.gcode", &cube, &args);
    args.extend(["--set", "mica_brick_layers=1"]);
    let (on, said) = slice(&unlocked, &dir, "on.gcode", &cube, &args);
    assert!(
        said.contains("Brick layers are off for this plate"),
        "{said}"
    );
    assert!(on == off);
    // One tool on the XL: brick layers apply.
    let mut one = vec![shells.as_str()];
    one.extend(xl);
    let (plain, _) = slice(&unlocked, &dir, "plain.gcode", &cube, &one);
    one.extend(["--set", "mica_brick_layers=1"]);
    let (bricked, said) = slice(&unlocked, &dir, "bricked.gcode", &cube, &one);
    assert!(!said.contains("Brick layers are off"), "{said}");
    let report = mica_gcode::brick_check::check_with(&bricked, &plain, 4, 6, false);
    assert!(
        report.problems.is_empty() && report.raised > 0,
        "{report:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hidden_unless_unlocked_and_identical_when_off() {
    let dir = temp("hidden");
    let plain = settings(&dir, false);
    let unlocked = settings(&dir, true);
    let cube = model("cube20.stl");
    let (reference, _) = slice(&plain, &dir, "reference.gcode", &cube, &[]);
    // Unlocked but not turned on: today's G-code.
    let (unused, quiet) = slice(&unlocked, &dir, "unused.gcode", &cube, &[]);
    assert!(reference == unused && !quiet.contains("Brick layers"));
    // Turned on in the settings without the opt-in: sliced without it, and said.
    let on = ["--set", "mica_brick_layers=1"];
    let (locked, said) = slice(&plain, &dir, "locked.gcode", &cube, &on);
    assert!(reference == locked, "brick layers apply only when unlocked");
    assert!(said.contains("This project uses Brick layers"), "{said}");
    let (bricked, _) = slice(&unlocked, &dir, "bricked.gcode", &cube, &on);
    assert!(reference != bricked);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Checks a pair of G-code files: `MICA_BRICK_ON`, `MICA_BRICK_OFF` (text G-code), shells
/// from `MICA_BRICK_SHELLS` ("bottom,top", default "4,6").
#[test]
#[ignore = "checks files named by environment variables"]
fn check_files() {
    let on = std::fs::read_to_string(std::env::var("MICA_BRICK_ON").unwrap()).unwrap();
    let off = std::fs::read_to_string(std::env::var("MICA_BRICK_OFF").unwrap()).unwrap();
    let shells = std::env::var("MICA_BRICK_SHELLS").unwrap_or_else(|_| "4,6".into());
    let (b, t) = shells.split_once(',').unwrap();
    let report = mica_gcode::brick_check::check(&on, &off, b.parse().unwrap(), t.parse().unwrap());
    println!("{report:#?}");
    assert!(report.problems.is_empty());
}
