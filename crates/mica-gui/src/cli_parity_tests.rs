// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The CLI/GUI invariant: for the same model files, printer/print/filament presets and
//! placement, the G-code the GUI exports is byte-identical to `mica slice`'s. The GUI side runs
//! headless (load, the editor's edits, the slice worker, Save); the CLI side is `mica_cli::run`,
//! the very code the `mica` binary runs. Both build the print with
//! `mica_core::input::prepare_plate`; these tests keep it that way.
//!
//! Also here: identical objects print identically wherever they stand (an editor copy moved
//! elsewhere prints the same paths, moved), as PrusaSlicer slices each object in its frame.
//!
//! The cases slice small models in the default (debug) test build. `cargo xtask parity` runs
//! the same invariant in release, including the `#[ignore]`d 3DBenchy cases.

/// An object and a copy the editor moved elsewhere (each prepared on its own, as separate
/// objects with other tools are) print the same paths on every layer, moved: the editor slices
/// both from the same model frame, in object coordinates.
#[test]
fn a_moved_copy_prints_the_same_moved() {
    use mica_core::print::LayerSource;
    let ctx = egui::Context::default();
    let Some(mut app) = gui(&ctx, &XL) else {
        return;
    };
    open(&mut app, &[models().join("hole_block.stl")]);
    let original = app.plates.current().objects[0].clone();
    let mut copy = original.fresh();
    copy.apply(Delta::Translate(Vec3::new(-71.317, 43.0091, 0.)));
    let config = app.plate_config(0).unwrap();
    let prepare = |o: &crate::scene::Object| {
        o.slicing_input(crate::worker::object_config(&config, o).unwrap())
            .prepare()
            .unwrap()
    };
    let (a, b) = (prepare(&original), prepare(&copy));
    let centre = |o: &mica_core::print_job::PrintObject| {
        let (min, max) = o.mesh.bounds();
        mica_core::geometry::Point::from_mm(
            (f64::from(min[0]) + f64::from(max[0])) / 2.0,
            (f64::from(min[1]) + f64::from(max[1])) / 2.0,
        )
    };
    let (ca, cb) = (centre(&a), centre(&b));
    let shift = mica_core::geometry::Point::new(cb.x - ca.x, cb.y - ca.y);
    let n = a.prepared.layer_count();
    assert_eq!(n, b.prepared.layer_count());
    let (pa, pb) = (a.prepared.toolpaths(0..n), b.prepared.toolpaths(0..n));
    for (i, (la, lb)) in pa.iter().zip(&pb).enumerate() {
        let mut moved = la.clone();
        mica_core::instances::shift_layer(&mut moved, shift);
        assert_eq!(moved.paths, lb.paths, "layer {}", i + 1);
    }
}

use crate::app::{App, Options};
use crate::scene::Delta;
use eframe::egui;
use glam::Vec3;
use mica_core::threemf::Flavour;
use mica_profiles::user::Store;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn models() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models/"))
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mica-parity-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// A printer with the presets the CLI names (`--printer`, `--print`, `--filament`, `--bed`).
struct Printer {
    vendor: &'static str,
    printer: &'static str,
    print: &'static str,
    filament: &'static str,
    bed: Option<&'static str>,
}

const CORE_ONE: Printer = Printer {
    vendor: "Prusa",
    printer: "Prusa CORE One HF0.4 nozzle",
    print: "0.20mm SPEED @COREONE HF0.4",
    filament: "Prusament PLA @COREONE HF0.4",
    bed: None,
};
const XL: Printer = Printer {
    vendor: "Prusa",
    printer: "Original Prusa XL - 5T Input Shaper 0.4 nozzle",
    print: "0.20mm SPEED @XLIS 0.4",
    filament: "Prusament PLA @XLIS",
    bed: None,
};
const MINI: Printer = Printer {
    vendor: "Prusa",
    printer: "Original Prusa MINI & MINI+ Input Shaper",
    print: "0.20mm SPEED @MINIIS 0.4",
    filament: "Prusament PLA @MINIIS",
    bed: None,
};
const MK3_5: Printer = Printer {
    vendor: "Prusa",
    printer: "Original Prusa MK3.5 & MK3.5S 0.4 nozzle",
    print: "0.20mm SPEED @MK3.5 0.4",
    filament: "Prusament PLA @MK3.5",
    bed: None,
};
const U1: Printer = Printer {
    vendor: "Snapmaker",
    printer: "Snapmaker U1 (0.4 nozzle)",
    print: "0.20 Standard @Snapmaker U1 (0.4 nozzle)",
    filament: "Snapmaker PLA @U1",
    bed: Some("High Temp Plate"),
};

impl Printer {
    /// The CLI's profile arguments; `presets: false` leaves the print and filament to a
    /// project's own settings (as the corpus slices `plates-orca.3mf`).
    fn args(&self, presets: bool) -> Vec<String> {
        let mut a = vec!["--vendor", self.vendor, "--printer", self.printer];
        if presets {
            a.extend(["--print", self.print, "--filament", self.filament]);
        }
        if let Some(bed) = self.bed {
            a.extend(["--bed", bed]);
        }
        a.into_iter().map(String::from).collect()
    }
}

/// A headless editor on `printer` with its presets; `None` (a reported skip, a failure with
/// MICA_REQUIRE_FIXTURES=1) when the pinned profiles of its vendor are not fetched.
fn gui(ctx: &egui::Context, printer: &Printer) -> Option<App> {
    gui_with(ctx, printer, None)
}

/// [`gui`] with the Snapmaker presets of the Orca `resources/profiles` directory `orca` (as
/// `--profiles-dir` gives the CLI) instead of the installed ones.
fn gui_with(ctx: &egui::Context, printer: &Printer, orca: Option<&Path>) -> Option<App> {
    if orca.is_none() && !mica_testkit::require_vendor(printer.vendor) {
        return None;
    }
    let dir = std::env::temp_dir().join(format!(
        "mica-parity-store-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let mut app = App::create(
        ctx,
        Options {
            prefs_path: None,
            store: Some(Store::open(dir)),
            files: vec![],
            headless: true,
        },
    );
    if let Some(root) = orca {
        let (bundle, _) =
            mica_profiles::orca::discovery::load_installed(Some(root), "Snapmaker").unwrap();
        let slot = app
            .profiles
            .vendors
            .iter_mut()
            .find(|(_, v)| matches!(v, crate::panels::profiles::Vendor::Orca(_)))
            .expect("an Orca vendor slot");
        slot.1 = crate::panels::profiles::Vendor::Orca(bundle);
    }
    let (vi, _, name) = app
        .profiles
        .all_printers()
        .into_iter()
        .find(|(_, _, n)| n == printer.printer)
        .unwrap_or_else(|| panic!("{} is not among the loaded printers", printer.printer));
    app.profiles.select_printer(vi, &name);
    app.profiles.process = printer.print.into();
    app.profiles.refresh();
    app.profiles.filaments.fill(printer.filament.into());
    if let Some(bed) = printer.bed {
        app.profiles.bed = bed.into();
    }
    app.profiles.refresh();
    app.settings_changed();
    assert_eq!(app.profiles.process, printer.print);
    assert_eq!(app.profiles.filaments[0], printer.filament);
    Some(app)
}

/// File › Open of `paths` (all at once, as a multi-file drop), with the project's own
/// settings applied as the GUI applies them for a matching printer.
fn open(app: &mut App, paths: &[PathBuf]) {
    let result = crate::app::load_files(paths.to_vec(), &app.config, app.bed.center());
    app.finish_load(result);
    assert!(!app.plates.is_blank(), "{}", app.status);
}

/// The GUI's G-code of plate `plate`: the slice worker on the editor's job and
/// configuration, as Export writes it.
fn gui_gcode(app: &App, plate: usize) -> Vec<u8> {
    gui_export(app, plate, "print.gcode")
}

fn gui_export(app: &App, plate: usize, filename: &str) -> Vec<u8> {
    let inputs = app.slice_inputs(plate).unwrap();
    let finished = match &inputs.job {
        crate::slice_cache::Job::Objects(objects, custom) => {
            crate::worker::slice_with_custom_gcode(
                objects.clone(),
                custom.clone(),
                inputs.config.clone(),
                &AtomicBool::new(false),
                &|_| {},
            )
        }
        crate::slice_cache::Job::Calibration(_) => panic!("not an object plate"),
    }
    .unwrap();
    std::fs::read(finished.preview.job.path.join(filename)).unwrap()
}

/// `mica slice <paths> <args> -o out.gcode` through the CLI's own entry point.
fn cli_gcode(paths: &[PathBuf], args: &[String]) -> Vec<u8> {
    cli_export(paths, args, "gcode")
}

fn cli_export(paths: &[PathBuf], args: &[String], encoding: &str) -> Vec<u8> {
    let out = temp(&format!("cli.{encoding}"));
    let mut all = vec!["slice".to_string()];
    all.extend(paths.iter().map(|p| p.to_string_lossy().into_owned()));
    all.extend(args.iter().cloned());
    all.extend(["-o".into(), out.to_string_lossy().into_owned()]);
    mica_cli::run(&all).unwrap_or_else(|e| panic!("mica {all:?}: {e}"));
    std::fs::read(&out).unwrap()
}

/// Byte-identical G-code; on a mismatch both files are kept for diffing and the first
/// differing line is reported.
fn same(gui: &[u8], cli: &[u8], what: &str) {
    let normalize = |bytes: &[u8]| {
        String::from_utf8_lossy(bytes)
            .lines()
            .filter(|l| !l.starts_with("; generated by "))
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    if gui == cli || normalize(gui) == normalize(cli) {
        return;
    }
    let dir = std::env::temp_dir();
    let name = what.replace([' ', '/'], "_");
    let (a, b) = (
        dir.join(format!("mica-parity-{name}-gui.gcode")),
        dir.join(format!("mica-parity-{name}-cli.gcode")),
    );
    std::fs::write(&a, gui).unwrap();
    std::fs::write(&b, cli).unwrap();
    let (ga, cb) = (String::from_utf8_lossy(gui), String::from_utf8_lossy(cli));
    let first = ga
        .lines()
        .zip(cb.lines())
        .enumerate()
        .find(|(_, (x, y))| x != y)
        .map(|(i, (x, y))| format!("line {}: GUI `{x}` / CLI `{y}`", i + 1))
        .unwrap_or_else(|| "one is a prefix of the other".into());
    panic!(
        "{what}: GUI and CLI G-code differ ({} / {} bytes), {first}; see {} and {}",
        gui.len(),
        cli.len(),
        a.display(),
        b.display()
    );
}

/// An STL opened in the GUI and sliced by the CLI with the same presets.
fn stl_case(printer: &Printer, files: &[&str]) {
    let ctx = egui::Context::default();
    let Some(mut app) = gui(&ctx, printer) else {
        return;
    };
    let paths: Vec<PathBuf> = files.iter().map(|f| models().join(f)).collect();
    open(&mut app, &paths);
    let what = format!("{} on {}", files.join("+"), printer.printer);
    same(
        &gui_gcode(&app, 0),
        &cli_gcode(&paths, &printer.args(true)),
        &what,
    );
}

#[test]
fn stl_on_the_core_one() {
    stl_case(&CORE_ONE, &["cube20.stl"]);
}

#[test]
fn stl_on_the_xl() {
    stl_case(&XL, &["cube20.stl"]);
}

#[test]
fn stl_on_the_mini() {
    stl_case(&MINI, &["cube20.stl"]);
}

#[test]
fn stl_on_the_mk3_5() {
    stl_case(&MK3_5, &["cube20.stl"]);
}

#[test]
fn stl_on_the_u1() {
    stl_case(&U1, &["cube20.stl"]);
}

#[test]
fn multi_object_plate_on_the_core_one() {
    stl_case(&CORE_ONE, &["cube20.stl", "hole_block.stl"]);
}

#[test]
fn multi_object_plate_on_the_u1() {
    stl_case(&U1, &["cube20.stl", "hole_block.stl"]);
}

#[test]
#[ignore = "release: cargo xtask parity"]
fn benchy_on_the_core_one() {
    stl_case(&CORE_ONE, &["3DBenchy.stl"]);
}

#[test]
#[ignore = "release: cargo xtask parity"]
fn benchy_on_the_xl() {
    stl_case(&XL, &["3DBenchy.stl"]);
}

#[test]
#[ignore = "release: cargo xtask parity"]
fn benchy_on_the_u1() {
    stl_case(&U1, &["3DBenchy.stl"]);
}

/// The other slicer's rules: the "Slicing rules" switch in the GUI's printer settings, and
/// `--slicing-rules` on the CLI (or, `saved`, the project the GUI saved, which records them:
/// in `Slic3r_PE.config` for a Prusa project, in Mica's sidecar for an Orca one). The
/// other engine's settings are filled in the same way on both sides.
fn rules_case(printer: &Printer, rules: mica_core::rules::Rules, saved: Option<Flavour>) {
    let ctx = egui::Context::default();
    let Some(mut app) = gui(&ctx, printer) else {
        return;
    };
    let paths = [models().join("cube20.stl")];
    open(&mut app, &paths);
    app.apply_edit(
        Some(crate::overlays::Target::Printer),
        crate::panels::editor::Edit::Set {
            key: mica_core::rules::KEY.into(),
            raw: rules.name().into(),
        },
    );
    assert_eq!(
        app.effective_config().map(mica_core::rules::Rules::of),
        Some(rules)
    );
    // The switch is an ordinary printer edit: unsaved marker and undo.
    assert!(app.overlays.printer.contains_key(mica_core::rules::KEY));
    let what = format!("{} rules on {}", rules.label(), printer.printer);
    let mut saved_file = None;
    let cli = match saved {
        None => {
            let mut args = printer.args(true);
            args.extend(["--slicing-rules".into(), rules.name().into()]);
            cli_gcode(&paths, &args)
        }
        Some(flavour) => {
            let file = temp("saved.3mf");
            app.save_project_to(&file, flavour).unwrap();
            saved_file = Some(file.clone());
            cli_gcode(&[file], &printer.args(false))
        }
    };
    if let Some(file) = saved_file {
        same_saved(&gui_gcode(&app, 0), &cli, &file, printer, None, &what);
    } else {
        same(&gui_gcode(&app, 0), &cli, &what);
    }
    app.undo();
    assert_eq!(
        app.effective_config().map(mica_core::rules::Rules::of),
        Some(app.profiles.profile_rules())
    );
}

/// Spiral vase turned on in the sidebar (which also sets one wall, no top, no infill and no
/// supports) and on the CLI with the same values: the same spiral, byte for byte. The hole
/// block's hole closes above its solid bottom.
fn spiral_vase_case(printer: &Printer) {
    let ctx = egui::Context::default();
    let Some(mut app) = gui(&ctx, printer) else {
        return;
    };
    let paths = [models().join("hole_block.stl")];
    open(&mut app, &paths);
    app.apply_edit(
        Some(crate::overlays::Target::Print),
        crate::panels::editor::Edit::Set {
            key: "spiral_vase".into(),
            raw: "1".into(),
        },
    );
    let config = app.effective_config().unwrap().clone();
    assert_eq!(config.int("perimeters"), Some(1));
    assert_eq!(config.bool("support_material"), Some(false));
    let mut args = printer.args(true);
    for key in [
        "spiral_vase",
        "perimeters",
        "top_solid_layers",
        "fill_density",
        "support_material",
        "support_material_enforce_layers",
        "thin_walls",
        "overhang_reverse",
    ] {
        if let Some(raw) = app.overlays.print.get(key) {
            args.extend(["--set".into(), format!("{key}={raw}")]);
        }
    }
    let gui = gui_gcode(&app, 0);
    let text = String::from_utf8_lossy(&gui);
    // Z rises within the spiral layers (G1 Z… X… Y… E…).
    assert!(
        text.lines()
            .filter(|l| l.starts_with("G1 Z") && l.contains(" E"))
            .count()
            > 100,
        "no spiral in the G-code"
    );
    same(
        &gui,
        &cli_gcode(&paths, &args),
        &format!("spiral vase on {}", printer.printer),
    );
}

#[test]
fn spiral_vase_on_the_core_one() {
    spiral_vase_case(&CORE_ONE);
}

#[test]
fn spiral_vase_on_the_u1() {
    spiral_vase_case(&U1);
}

#[test]
fn orca_rules_on_the_core_one() {
    rules_case(&CORE_ONE, mica_core::rules::Rules::OrcaSlicer, None);
}

#[test]
fn prusa_rules_on_the_u1() {
    rules_case(&U1, mica_core::rules::Rules::PrusaSlicer, None);
}

#[test]
fn orca_rules_saved_for_the_cli_on_the_core_one() {
    rules_case(
        &CORE_ONE,
        mica_core::rules::Rules::OrcaSlicer,
        Some(Flavour::Prusa),
    );
}

#[test]
fn prusa_rules_saved_for_the_cli_on_the_u1() {
    rules_case(
        &U1,
        mica_core::rules::Rules::PrusaSlicer,
        Some(Flavour::Orca),
    );
}

/// Cross-format Save may change native percentages/vector lengths and persists the
/// display palette as project colours. Check unchanged commands across that transition,
/// then check every byte with both frontends opening exactly the same saved settings.
fn same_saved(
    before: &[u8],
    cli: &[u8],
    file: &Path,
    printer: &Printer,
    profiles: Option<&Path>,
    what: &str,
) {
    let program = |g: &[u8]| {
        String::from_utf8_lossy(g)
            .split("; CONFIG_BLOCK_START")
            .next()
            .unwrap()
            .split("; prusaslicer_config = begin")
            .next()
            .unwrap()
            .lines()
            .filter(|l| !l.starts_with("; generated by "))
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        program(before),
        program(cli),
        "{what}: commands changed on Save"
    );
    let ctx = egui::Context::default();
    let mut reopened = gui_with(&ctx, printer, profiles).unwrap();
    open(&mut reopened, &[file.to_path_buf()]);
    same(&gui_gcode(&reopened, 0), cli, what);
}

/// Moves, turns and scales the first object as the editor's gizmos do.
fn edit(app: &mut App) {
    app.record("Edit");
    let o = &mut app.plates.current_mut().objects[0];
    o.apply(Delta::Translate(Vec3::new(-23.4567, 11.125, 0.)));
    o.apply(Delta::Rotate {
        axis: Vec3::Z,
        angle: 0.6,
    });
    o.apply(Delta::Scale(Vec3::new(1.25, 0.8, 1.1)));
    app.changed();
}

/// The GUI's export of an edited plate equals the CLI's slice of the project the GUI saved.
fn saved_case(printer: &Printer, file: &str, flavour: Flavour, edited: bool, args: &[&str]) {
    let ctx = egui::Context::default();
    let Some(mut app) = gui(&ctx, printer) else {
        return;
    };
    open(&mut app, &[models().join(file)]);
    if edited {
        edit(&mut app);
    }
    let saved = temp("saved.3mf");
    app.save_project_to(&saved, flavour).unwrap();
    // The saved project alone carries the print and filament settings.
    let mut cli_args = printer.args(false);
    cli_args.extend(args.iter().map(|s| s.to_string()));
    same_saved(
        &gui_gcode(&app, 0),
        &cli_gcode(std::slice::from_ref(&saved), &cli_args),
        &saved,
        printer,
        None,
        &format!("{file} saved as {flavour:?} on {}", printer.printer),
    );
}

#[test]
fn moved_turned_scaled_stl_saved_for_the_cli_on_the_core_one() {
    saved_case(&CORE_ONE, "cube20.stl", Flavour::Prusa, true, &[]);
}

#[test]
fn moved_turned_scaled_stl_saved_for_the_cli_on_the_u1() {
    saved_case(&U1, "cube20.stl", Flavour::Orca, true, &[]);
}

#[test]
fn unmoved_stl_saved_for_the_cli_on_the_xl() {
    saved_case(&XL, "hole_block.stl", Flavour::Prusa, false, &[]);
}

/// A project opened in the GUI (its settings applied as the GUI applies them) and sliced by
/// the CLI with `--project-settings` left to its default.
fn project_case(
    printer: &Printer,
    file: &str,
    plate: usize,
    presets: bool,
    gui_edits: &[(&str, &str)],
) {
    let ctx = egui::Context::default();
    let Some(mut app) = gui(&ctx, printer) else {
        return;
    };
    let path = models().join(file);
    open(&mut app, std::slice::from_ref(&path));
    app.select_plate(plate);
    // An instance the project assigns to no plate joins the nearest one in the editor, beside
    // its bed, where the GUI refuses to slice it; the CLI leaves it out of every plate.
    let bed = app.bed.clone();
    app.plates.current_mut().objects.retain(|o| {
        let (min, max) = o.bounds();
        min.x < bed.max.x && max.x > bed.min.x && min.y < bed.max.y && max.y > bed.min.y
    });
    let mut args = printer.args(presets);
    args.extend(["--plate".into(), (plate + 1).to_string()]);
    for (k, v) in gui_edits {
        app.overlays.print.insert(k.to_string(), v.to_string());
        args.extend(["--set".into(), format!("{k}={v}")]);
    }
    app.settings_changed();
    same(
        &gui_gcode(&app, plate),
        &cli_gcode(&[path], &args),
        &format!("{file} plate {} on {}", plate + 1, printer.printer),
    );
}

#[test]
fn painted_project_on_the_xl() {
    project_case(&XL, "painted-cube.3mf", 0, true, &[("wipe_tower", "1")]);
}

#[test]
fn orca_project_on_the_u1() {
    project_case(&U1, "plates-orca.3mf", 0, false, &[("wipe_tower", "1")]);
}

#[test]
fn orca_project_second_plate_on_the_u1() {
    project_case(&U1, "plates-orca.3mf", 1, false, &[]);
}

#[test]
fn orca_project_on_the_xl() {
    project_case(&XL, "plates-orca.3mf", 0, true, &[]);
}

#[test]
fn edited_painted_project_saved_for_the_cli_on_the_xl() {
    saved_case(&XL, "painted-cube.3mf", Flavour::Prusa, true, &[]);
}

#[test]
fn edited_orca_project_saved_for_the_cli_on_the_u1() {
    saved_case(
        &U1,
        "plates-orca.3mf",
        Flavour::Orca,
        true,
        &["--plate", "1"],
    );
}

/// Arrange moves the objects; the saved project reopens to the very same model frames (bit for
/// bit) and G-code, and the CLI slices that project to the same G-code too.
#[test]
fn arranged_plate_round_trips_exactly() {
    let ctx = egui::Context::default();
    let Some(mut app) = gui(&ctx, &CORE_ONE) else {
        return;
    };
    open(
        &mut app,
        &[models().join("cube20.stl"), models().join("hole_block.stl")],
    );
    let before: Vec<_> = app
        .plates
        .current()
        .objects
        .iter()
        .map(|o| o.model_frame().trafo)
        .collect();
    app.arrange();
    let frames: Vec<_> = app
        .plates
        .current()
        .objects
        .iter()
        .map(|o| o.model_frame())
        .collect();
    assert_ne!(
        before,
        frames.iter().map(|f| f.trafo).collect::<Vec<_>>(),
        "arrange moved nothing"
    );
    let gcode = gui_gcode(&app, 0);
    for flavour in [Flavour::Prusa, Flavour::Orca] {
        let saved = temp("arranged.3mf");
        app.save_project_to(&saved, flavour).unwrap();
        let mut again = gui(&ctx, &CORE_ONE).unwrap();
        open(&mut again, std::slice::from_ref(&saved));
        let reopened = &again.plates.current().objects;
        assert_eq!(reopened.len(), frames.len());
        for (a, b) in frames.iter().zip(reopened) {
            let b = b.model_frame();
            assert_eq!(a.trafo, b.trafo, "{flavour:?}: instance transform");
            assert_eq!(a.source.vertices, b.source.vertices, "{flavour:?}: mesh");
            assert_eq!(a.source.faces, b.source.faces, "{flavour:?}: mesh");
        }
        same(
            &gcode,
            &gui_gcode(&again, 0),
            &format!("arranged reopened {flavour:?}"),
        );
        same(
            &gcode,
            &cli_gcode(&[saved], &CORE_ONE.args(false)),
            &format!("arranged saved {flavour:?}"),
        );
    }
}

#[test]
fn painted_project_saved_for_the_cli_on_the_xl() {
    saved_case(&XL, "painted-cube.3mf", Flavour::Prusa, false, &[]);
}

#[test]
fn orca_project_saved_for_the_cli_on_the_u1() {
    saved_case(
        &U1,
        "plates-orca.3mf",
        Flavour::Orca,
        false,
        &["--plate", "1"],
    );
}

/// Snapmaker Orca's own profile set (the pinned fork), whose presets declare flow variants
/// (`filament_flow_support`); `None` (a reported skip) when it is not fetched.
fn snapmaker_profiles() -> Option<PathBuf> {
    mica_testkit::require_snapmaker_orca_profiles().then(mica_testkit::snapmaker_orca_profiles)
}

/// The U1 on Snapmaker Orca's profile set with a filament that has standard and high-flow
/// variants. Both sides detect the set's format with `discovery::load_installed`.
const U1_SNAPMAKER_FORMAT: Printer = Printer {
    print: "0.20mm Standard @Snapmaker U1 (0.4 nozzle)",
    filament: "Snapmaker PLA Matte @U1",
    ..U1
};

/// GUI and CLI resolve the same configuration from a Snapmaker-format profile set (a
/// flow-variant U1 filament on every tool): the GUI's slice, and the project it saves for
/// Snapmaker Orca, equal the CLI's with `--profiles-dir`.
#[test]
fn snapmaker_format_flow_variant_filament_on_the_u1() {
    let Some(root) = snapmaker_profiles() else {
        return;
    };
    let printer = &U1_SNAPMAKER_FORMAT;
    let ctx = egui::Context::default();
    let mut app = gui_with(&ctx, printer, Some(&root)).expect("U1 in the Snapmaker set");
    assert_eq!(
        app.profiles.orca_dialect(),
        Some(mica_profiles::orca::Dialect::Snapmaker240)
    );
    let dir = ["--profiles-dir".to_string(), root.display().to_string()];
    let paths = vec![models().join("cube20.stl")];
    open(&mut app, &paths);
    let mut args = printer.args(true);
    args.extend(dir.iter().cloned());
    let gui = gui_gcode(&app, 0);
    same(
        &gui,
        &cli_gcode(&paths, &args),
        "flow-variant filament on the U1",
    );
    // Saved for Snapmaker Orca and sliced from the project alone.
    let saved = temp("saved.3mf");
    app.save_project_to(&saved, Flavour::Orca).unwrap();
    let mut args = printer.args(false);
    args.extend(dir.iter().cloned());
    same_saved(
        &gui,
        &cli_gcode(std::slice::from_ref(&saved), &args),
        &saved,
        printer,
        Some(&root),
        "flow-variant filament saved as Orca on the U1",
    );
}

/// A RatRig V-Core 4 picked in the Add printer gallery (its vendor loaded
/// only then), added slice-only, sliced with the default process and filament Mica picks.
/// Slices under OrcaSlicer rules (the profile is Orca's) with Klipper G-code, byte-identical to
/// `mica slice --vendor Ratrig` with the same presets. A Voron and a Creality K1 likewise.
#[test]
fn gallery_printers_of_other_brands_add_and_slice_like_the_cli() {
    for (model, preset) in [
        ("RatRig V-Core 4", "RatRig V-Core 4 300 0.4 nozzle"),
        ("Voron 2.4", "Voron 2.4 250 0.4 nozzle"),
        ("Creality K1", "Creality K1 (0.4 nozzle)"),
    ] {
        gallery_printer_case(model, preset);
    }
}

fn gallery_printer_case(model: &str, preset: &str) {
    if !mica_testkit::require_orca_profiles() {
        return;
    }
    let ctx = egui::Context::default();
    let dir = std::env::temp_dir().join(format!(
        "mica-parity-store-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let mut app = App::create(
        &ctx,
        Options {
            prefs_path: None,
            store: Some(Store::open(dir)),
            files: vec![],
            headless: true,
        },
    );
    assert!(app.profiles.orca_root.is_some(), "no OrcaSlicer profiles");
    app.open_add_printer();
    app.load_gallery_index_now();
    let catalog = app.catalog().clone();
    let mi = catalog
        .models
        .iter()
        .position(|m| m.name == model)
        .unwrap_or_else(|| panic!("no {model} in the gallery"));
    let m = &catalog.models[mi];
    let key = m.vendor.clone();
    // Listed without loading the vendor.
    assert!(
        app.profiles.vendor_index(&key).is_none(),
        "{key} loaded early"
    );
    app.begin_setup(mi, None, None, crate::panels::add_printer::Screen::Gallery);
    let choice = app.add_printer.setup.as_ref().unwrap().choice.clone();
    assert_eq!(m.resolve(&choice).unwrap().preset, preset);
    app.finish_vendor_load();
    app.finish_setup();
    assert!(
        app.prefs.has_printer(&key, preset),
        "{:?} {:?}",
        app.prefs.my_printers,
        app.add_printer.error
    );
    assert_eq!(app.profiles.vendor_name(), key);
    assert_eq!(app.profiles.printer, preset);
    assert!(app.printers.list.is_empty(), "slice only");
    assert!(!app.profiles.process.is_empty() && !app.profiles.filaments[0].is_empty());
    assert_eq!(
        app.profiles.profile_rules(),
        mica_core::rules::Rules::OrcaSlicer
    );
    let config = app.profiles.config().unwrap();
    assert!(mica_core::rules::Rules::of(&config).is_orca(), "{preset}");
    assert_eq!(config.string("gcode_flavor"), Some("klipper"), "{preset}");
    let paths = vec![models().join("cube20.stl")];
    open(&mut app, &paths);
    let gui = gui_gcode(&app, 0);
    let text = String::from_utf8_lossy(&gui);
    // Orca's header, Klipper's commands (acceleration limits, object exclusion).
    assert!(text.starts_with("; HEADER_BLOCK_START"), "{preset}");
    assert!(text.contains("SET_VELOCITY_LIMIT ACCEL="), "{preset}");
    let args: Vec<String> = [
        "--vendor",
        key.as_str(),
        "--printer",
        preset,
        "--print",
        app.profiles.process.as_str(),
        "--filament",
        app.profiles.filaments[0].as_str(),
    ]
    .into_iter()
    .map(String::from)
    .chain(
        app.profiles
            .orca_root
            .iter()
            .flat_map(|r| ["--profiles-dir".to_string(), r.display().to_string()]),
    )
    .collect();
    same(
        &gui,
        &cli_gcode(&paths, &args),
        &format!("cube20 on {preset}"),
    );
}

/// A PrusaSlicer printer saved as an OrcaSlicer project, with edits to the settings whose
/// PrusaSlicer and Orca spellings differ (an enum that is a switch in Orca, Orca's own
/// vocabulary, "mm/s or %" values Orca only has in mm/s, the bed temperature Orca keeps per
/// plate type), reopens in the CLI without a rejected value and slices byte-identically.
fn orca_saved_prusa_case(printer: &Printer) {
    use crate::overlays::Target;
    use crate::panels::editor::Edit;
    let ctx = egui::Context::default();
    let Some(mut app) = gui(&ctx, printer) else {
        return;
    };
    open(&mut app, &[models().join("shells.stl")]);
    // Upside down: the pyramid's steps and the cone overhang, so supports print.
    app.record("Flip");
    app.plates.current_mut().objects[0].apply(Delta::Rotate {
        axis: Vec3::X,
        angle: std::f32::consts::PI,
    });
    app.changed();
    for (target, key, raw) in [
        (Target::Print, "arc_fitting", "emit_center"),
        (Target::Print, "ensure_vertical_shell_thickness", "partial"),
        (Target::Print, "top_one_perimeter_type", "topmost"),
        (Target::Print, "support_material", "1"),
        (Target::Print, "support_material_auto", "1"),
        (Target::Print, "support_material_speed", "45"),
        (Target::Print, "support_material_interface_speed", "80%"),
        (Target::Print, "support_material_xy_spacing", "60%"),
        (Target::Filament(0), "bed_temperature", "67"),
        (Target::Filament(0), "first_layer_bed_temperature", "71"),
    ] {
        app.apply_edit(
            Some(target),
            Edit::Set {
                key: key.into(),
                raw: raw.into(),
            },
        );
        app.end_gesture();
    }
    let config = &app.effective_config().unwrap().clone();
    assert_eq!(config.string("arc_fitting"), Some("emit_center"));
    let saved = temp("saved-orca.3mf");
    app.save_project_to(&saved, Flavour::Orca).unwrap();
    let raw = mica_core::threemf::load_config(&saved).unwrap().unwrap();
    assert!(
        raw.path.ends_with("project_settings.config"),
        "{}",
        raw.path
    );
    let settings = mica_profiles::project::ProjectSettings::parse(&raw).unwrap();
    let base = app.profiles.config().unwrap();
    let (layer, notes) = settings.layer(&base, None, mica_profiles::project::Mode::Embedded);
    assert!(notes.is_empty(), "{}: {notes:#?}", printer.printer);
    // The project reads back as the editor's values (percentages as what they slice with).
    let mut read = base.clone();
    layer.apply(&mut read);
    for key in [
        "arc_fitting",
        "ensure_vertical_shell_thickness",
        "top_one_perimeter_type",
        "support_material",
        "support_material_auto",
    ] {
        assert_eq!(read.raw(key), config.raw(key), "{key}");
    }
    for key in [
        "support_material_interface_speed",
        "support_material_xy_spacing",
        "bed_temperature",
        "first_layer_bed_temperature",
    ] {
        let value = |c: &mica_core::config::Config| {
            mica_core::resolve::absolute_value(c, key).unwrap_or_else(|_| c.float(key).unwrap())
        };
        assert_eq!(value(&read), value(config), "{key}");
    }
    let gcode = gui_gcode(&app, 0);
    assert!(
        String::from_utf8_lossy(&gcode).contains(";TYPE:Support material interface"),
        "the edited support prints"
    );
    same_saved(
        &gcode,
        &cli_gcode(std::slice::from_ref(&saved), &printer.args(false)),
        &saved,
        printer,
        None,
        &format!("edited shells saved as Orca on {}", printer.printer),
    );
}

#[test]
fn edited_prusa_printer_saved_as_orca_for_the_cli_on_the_core_one() {
    orca_saved_prusa_case(&CORE_ONE);
}

#[test]
fn edited_prusa_printer_saved_as_orca_for_the_cli_on_the_xl() {
    orca_saved_prusa_case(&XL);
}

/// SOURCE_DATE_EPOCH fixes both the text identity and binary Produced on metadata.
/// The xtask always provides it; do not mutate process-global environment in tests.
#[test]
#[ignore = "fixed-clock binary byte parity: cargo xtask parity (sets SOURCE_DATE_EPOCH=0)"]
fn binary_outputs_match_with_the_same_timestamp() {
    assert_eq!(
        std::env::var("SOURCE_DATE_EPOCH").as_deref(),
        Ok("0"),
        "run with SOURCE_DATE_EPOCH=0 (cargo xtask parity does)"
    );
    for printer in [&CORE_ONE, &XL] {
        let ctx = egui::Context::default();
        let Some(mut app) = gui(&ctx, printer) else {
            continue;
        };
        let paths = [models().join("cube20.stl")];
        open(&mut app, &paths);
        let a = gui_export(&app, 0, "print.bgcode");
        let b = cli_export(&paths, &printer.args(true), "bgcode");
        if a != b {
            let path = temp("gui.bgcode");
            std::fs::write(&path, &a).unwrap();
            std::fs::write(path.with_file_name("cli.bgcode"), &b).unwrap();
            panic!(
                "binary GUI/CLI mismatch on {}; see {}",
                printer.printer,
                path.display()
            );
        }
    }
}

/// Mixed colors: one object on an Outer wall first mix, another on a gradient mix, saved and
/// sliced by the CLI from the saved project, byte for byte; both physical tools print.
fn mixes_case(printer: &Printer, flavour: Flavour) {
    let ctx = egui::Context::default();
    let Some(mut app) = gui(&ctx, printer) else {
        return;
    };
    open(
        &mut app,
        &[models().join("cube20.stl"), models().join("hole_block.stl")],
    );
    app.add_mix_spec("1:75,2:25/outer").unwrap();
    app.add_mix_spec("1:80,2:20/gradient:20,80@Sunset").unwrap();
    let physical = app.tool_count();
    assert_eq!(app.mixes.list.len(), 2);
    assert_eq!(app.mix_name(1), "Sunset");
    app.plates.current_mut().objects[0].set_tool(physical);
    app.plates.current_mut().objects[1].set_tool(physical + 1);
    app.changed();
    let saved = temp("mixes.3mf");
    app.save_project_to(&saved, flavour).unwrap();
    let gui_out = gui_gcode(&app, 0);
    let text = String::from_utf8_lossy(&gui_out);
    for t in ["T0", "T1"] {
        assert!(
            text.lines()
                .any(|l| l.trim() == t || l.starts_with(&format!("{t} "))),
            "{t} missing"
        );
    }
    same_saved(
        &gui_out,
        &cli_gcode(std::slice::from_ref(&saved), &printer.args(false)),
        &saved,
        printer,
        None,
        &format!("mixed colors saved as {flavour:?} on {}", printer.printer),
    );
    // The saved project brings the mixes back, numbered and assigned as they were.
    let mut reopened = gui(&ctx, printer).unwrap();
    open(&mut reopened, &[saved]);
    assert_eq!(reopened.mixes.list.len(), 2);
    assert_eq!(reopened.mix_name(1), "Sunset");
    assert_eq!(
        reopened.plates.current().objects[1].tools.0[0],
        Some(physical + 1)
    );
}

#[test]
fn mixed_colors_saved_for_the_cli_on_the_xl() {
    mixes_case(&XL, Flavour::Prusa);
}

#[test]
fn mixed_colors_saved_for_the_cli_on_the_u1() {
    mixes_case(&U1, Flavour::Orca);
}

/// A Moonraker printer's File header setting feeds the slice like the CLI's
/// `--gcode-identity`: same G-code, same identity line (Automatic with nothing learned: the
/// profile's default, as the CLI without the flag).
#[test]
fn printer_file_header_matches_the_cli_gcode_identity() {
    use crate::printhost::config::{FileHeader, Kind, Printer as Host};
    let ctx = egui::Context::default();
    let Some(mut app) = gui(&ctx, &U1) else {
        return;
    };
    let paths = [models().join("cube20.stl")];
    open(&mut app, &paths);
    let mut host = Host::new(Kind::SnapmakerU1);
    host.name = "U1".into();
    host.address = "127.0.0.1:1".into();
    host.profile = Some(format!(
        "{}/{}",
        app.profiles.vendor_name(),
        app.profiles.printer
    ));
    let i = app.printers.put(None, host, None).unwrap();
    assert_eq!(app.linked_printer(), Some(i));
    // The identity line up to its timestamp.
    let identity = |g: &[u8]| {
        String::from_utf8_lossy(g)
            .lines()
            .find(|l| l.starts_with("; generated by "))
            .map(|l| l.split(" on ").next().unwrap().to_string())
            .unwrap()
    };
    for (header, detected, flag, want) in [
        (
            None,
            None,
            None,
            "; generated by Mica Slicer 0.0.1 (OrcaSlicer 2.4.2",
        ),
        (
            Some(FileHeader::Mica),
            None,
            Some("mica"),
            "; generated by Mica Slicer 0.0.1",
        ),
        (
            None,
            Some(FileHeader::OrcaCompatible),
            Some("orca-compatible"),
            "; generated by Mica Slicer 0.0.1 (OrcaSlicer 2.4.2",
        ),
    ] {
        app.printers.list[i].file_header = header;
        app.printers.list[i].file_header_detected = detected;
        // What the app does every frame: a changed header recomposes the configuration.
        app.check_file_identity();
        let a = gui_gcode(&app, 0);
        let mut args = U1.args(true);
        if let Some(f) = flag {
            args.extend(["--gcode-identity".into(), f.into()]);
        }
        let b = cli_gcode(&paths, &args);
        let what = format!("U1 file header {header:?}/{detected:?}");
        assert_eq!(identity(&a), want, "{what}");
        assert_eq!(identity(&a), identity(&b), "{what}");
        same(&a, &b, &what);
    }
}

/// Texture skin with a custom image: the GUI's texture settings and imported image slice like
/// `mica slice --texture-image … --set …`, and the project the GUI saves (image embedded by
/// its hash) slices the same on the CLI.
fn texture_case(printer: &Printer, flavour: Flavour) {
    let ctx = egui::Context::default();
    let Some(mut app) = gui(&ctx, printer) else {
        return;
    };
    let image = temp("texture.png");
    let stripes = mica_core::texture::heightmap::HeightMap::new(
        16,
        8,
        (0..128).map(|i| ((i % 16) as f32 / 15.).powi(2)).collect(),
    )
    .unwrap();
    std::fs::write(&image, stripes.to_png()).unwrap();
    let value = crate::textures::import_image(&image).unwrap();
    let paths = [models().join("cube20.stl")];
    open(&mut app, &paths);
    let settings = [
        ("mica_texture_skin", "external"),
        ("mica_texture_skin_scale", "4"),
        ("mica_texture_skin_direction", "both"),
    ];
    for (key, raw) in settings
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .chain([(crate::textures::TEXTURE_KEY.to_string(), value)])
    {
        app.apply_edit(
            Some(crate::overlays::Target::Print),
            crate::panels::editor::Edit::Set { key, raw },
        );
    }
    let mut args = printer.args(true);
    args.extend([
        "--texture-image".into(),
        image.to_string_lossy().into_owned(),
    ]);
    for (k, v) in settings {
        args.extend(["--set".into(), format!("{k}={v}")]);
    }
    let what = format!("texture skin on {}", printer.printer);
    let gui = gui_gcode(&app, 0);
    same(&gui, &cli_gcode(&paths, &args), &what);
    let file = temp("textured.3mf");
    app.save_project_to(&file, flavour).unwrap();
    same_saved(
        &gui,
        &cli_gcode(std::slice::from_ref(&file), &printer.args(false)),
        &file,
        printer,
        None,
        &format!("{what}, saved as {flavour:?}"),
    );
}

#[test]
fn texture_skin_with_an_image_on_the_core_one() {
    texture_case(&CORE_ONE, Flavour::Prusa);
}

#[test]
fn texture_skin_with_an_image_on_the_u1() {
    texture_case(&U1, Flavour::Orca);
}

/// Texture on all surfaces (the mesh displaced before slicing): the GUI and `mica slice
/// --set …` slice alike, and so does the project the GUI saves.
fn texture_all_surfaces_case(printer: &Printer, flavour: Flavour) {
    let ctx = egui::Context::default();
    let Some(mut app) = gui(&ctx, printer) else {
        return;
    };
    let paths = [models().join("cube20.stl")];
    open(&mut app, &paths);
    let plain = gui_gcode(&app, 0);
    let settings = [
        ("mica_texture_skin", "external"),
        ("mica_texture_skin_surfaces", "walls_and_tops"),
        ("mica_texture_skin_texture", "builtin:shingles"),
        ("mica_texture_skin_scale", "5"),
    ];
    for (key, raw) in settings {
        app.apply_edit(
            Some(crate::overlays::Target::Print),
            crate::panels::editor::Edit::Set {
                key: key.into(),
                raw: raw.into(),
            },
        );
    }
    let mut args = printer.args(true);
    for (k, v) in settings {
        args.extend(["--set".into(), format!("{k}={v}")]);
    }
    let what = format!("texture on all surfaces on {}", printer.printer);
    let gui = gui_gcode(&app, 0);
    assert_ne!(gui, plain, "{what}: textured");
    same(&gui, &cli_gcode(&paths, &args), &what);
    let file = temp("textured-all.3mf");
    app.save_project_to(&file, flavour).unwrap();
    same_saved(
        &gui,
        &cli_gcode(std::slice::from_ref(&file), &printer.args(false)),
        &file,
        printer,
        None,
        &format!("{what}, saved as {flavour:?}"),
    );
}

#[test]
fn texture_on_all_surfaces_on_the_core_one() {
    texture_all_surfaces_case(&CORE_ONE, Flavour::Prusa);
}

#[test]
fn texture_on_all_surfaces_on_the_u1() {
    texture_all_surfaces_case(&U1, Flavour::Orca);
}

/// Texture paint with a slot's own depth (and the slot on all surfaces): the GUI's slice equals
/// the CLI's slice of the project it saves, and the depth changes the result.
#[test]
fn painted_slot_depth_slices_alike() {
    let ctx = egui::Context::default();
    let Some(mut app) = gui(&ctx, &CORE_ONE) else {
        return;
    };
    open(&mut app, &[models().join("cube20.stl")]);
    app.select(Some(0), false);
    {
        let o = &mut app.plates.current_mut().objects[0];
        let mesh = o.mesh.clone();
        let painting = std::sync::Arc::make_mut(&mut o.painting);
        // Every upward- and +X-facing triangle with slot 1.
        for (i, f) in mesh.faces.iter().enumerate() {
            let [a, b, c] = f.map(|v| glam::Vec3::from_array(mesh.vertices[v as usize]));
            let n = (b - a).cross(c - a).normalize();
            if n.z > 0.9 || n.x > 0.9 {
                painting.texture.push(i as u32, "4").unwrap();
            }
        }
    }
    app.changed();
    let mut gcode = Vec::new();
    for slots in [
        "1:texture=builtin:shingles,scale=5",
        "1:texture=builtin:shingles,scale=5,amplitude=0.15",
        "1:texture=builtin:shingles,scale=5,amplitude=0.15,surfaces=walls_and_tops",
    ] {
        app.apply_edit(
            None,
            crate::panels::editor::Edit::Set {
                key: crate::textures::SLOTS_KEY.into(),
                raw: slots.into(),
            },
        );
        let gui = gui_gcode(&app, 0);
        let file = temp("slot-depth.3mf");
        app.save_project_to(&file, Flavour::Prusa).unwrap();
        let what = format!("painted slot {slots}");
        same_saved(
            &gui,
            &cli_gcode(std::slice::from_ref(&file), &CORE_ONE.args(false)),
            &file,
            &CORE_ONE,
            None,
            &what,
        );
        gcode.push(gui);
    }
    assert_ne!(gcode[0], gcode[1], "the slot's own depth counts");
    assert_ne!(gcode[1], gcode[2], "the slot textures all surfaces");
}

/// `mica slice` with these Settings › Features switches (as the settings folder's
/// mica.toml would give them).
fn cli_gcode_with(paths: &[PathBuf], args: &[String], features: crate::state::Features) -> Vec<u8> {
    let out = temp("cli-features.gcode");
    let mut all: Vec<String> = paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    all.extend(args.iter().cloned());
    all.extend(["-o".into(), out.to_string_lossy().into_owned()]);
    mica_cli::slice_with_features(&all, features)
        .unwrap_or_else(|e| panic!("mica slice {all:?}: {e}"));
    std::fs::read(&out).unwrap()
}

fn uses_tool(gcode: &[u8], t: &str) -> bool {
    String::from_utf8_lossy(gcode)
        .lines()
        .any(|l| l.trim() == t || l.starts_with(&format!("{t} ")))
}

/// ColorMix off in Settings › Features: a project with mixed colors says so (with Turn
/// on), prints every mix in its main color in the GUI and the CLI alike, and keeps its mixes
/// in the saved project.
#[test]
fn color_mix_off_prints_main_colors_and_keeps_the_mixes() {
    use crate::state::Feature;
    let ctx = egui::Context::default();
    let Some(mut app) = gui(&ctx, &XL) else {
        return;
    };
    open(
        &mut app,
        &[models().join("cube20.stl"), models().join("hole_block.stl")],
    );
    app.add_mix_spec("1:25,2:75/outer").unwrap();
    app.add_mix_spec("1:80,2:20/gradient:20,80@Sunset").unwrap();
    let physical = app.tool_count();
    app.plates.current_mut().objects[0].set_tool(physical);
    app.plates.current_mut().objects[1].set_tool(physical + 1);
    app.changed();
    let mixed = gui_gcode(&app, 0);
    assert!(uses_tool(&mixed, "T0") && uses_tool(&mixed, "T1"));
    assert!(app.features_off_in_use().is_empty());

    app.set_feature(Feature::ColorMix, false);
    assert_eq!(app.features_off_in_use(), vec![Feature::ColorMix]);
    // Only the physical tools are offered; the mixes show in their main colors.
    assert_eq!(app.tool_choices().len(), physical);
    let off = gui_gcode(&app, 0);
    assert_ne!(off, mixed);
    // 25/75 is mainly tool 2, the gradient starts mainly on tool 1.
    assert!(uses_tool(&off, "T0") && uses_tool(&off, "T1"));
    let saved = temp("mixes-off.3mf");
    app.save_project_to(&saved, Flavour::Prusa).unwrap();
    let features = crate::state::Features {
        color_mix: false,
        ..Default::default()
    };
    let cli_off = cli_gcode_with(
        std::slice::from_ref(&saved),
        &XL.args(false),
        features.clone(),
    );
    // The saved project still mixes with ColorMix on.
    let cli_on = cli_gcode(std::slice::from_ref(&saved), &XL.args(false));
    assert_ne!(cli_on, cli_off);
    let mut reopened = gui(&ctx, &XL).unwrap();
    reopened.prefs.features = features;
    open(&mut reopened, &[saved]);
    assert_eq!(
        reopened.mixes.list.len(),
        2,
        "the mixes stay in the project"
    );
    assert_eq!(reopened.mix_name(1), "Sunset");
    same(
        &gui_gcode(&reopened, 0),
        &cli_off,
        "ColorMix off, saved project",
    );
    // Turn on (the notice's action) brings the mixes back.
    reopened.run_notification_action(
        0,
        crate::notifications::Action::EnableFeature(Feature::ColorMix),
        &ctx,
    );
    assert!(reopened.prefs.features.color_mix);
    assert!(reopened.features_off_in_use().is_empty());
    same(&gui_gcode(&reopened, 0), &cli_on, "ColorMix on again");
}

/// Textures off in Settings › Features: the textured project slices without its texture
/// in the GUI and the CLI alike, says so, and keeps its texture settings in the saved project.
#[test]
fn textures_off_slice_alike_and_keep_the_settings() {
    use crate::state::Feature;
    let ctx = egui::Context::default();
    let Some(mut app) = gui(&ctx, &CORE_ONE) else {
        return;
    };
    let paths = [models().join("cube20.stl")];
    open(&mut app, &paths);
    let plain = gui_gcode(&app, 0);
    for (key, raw) in [
        ("mica_texture_skin", "external"),
        ("mica_texture_skin_texture", "builtin:shingles"),
    ] {
        app.apply_edit(
            Some(crate::overlays::Target::Print),
            crate::panels::editor::Edit::Set {
                key: key.into(),
                raw: raw.into(),
            },
        );
    }
    let textured = gui_gcode(&app, 0);
    assert_ne!(plain, textured);
    app.set_feature(Feature::Textures, false);
    assert_eq!(app.features_off_in_use(), vec![Feature::Textures]);
    let off = gui_gcode(&app, 0);
    let features = crate::state::Features {
        textures: false,
        ..Default::default()
    };
    let mut args = CORE_ONE.args(true);
    args.extend([
        "--set".into(),
        "mica_texture_skin=external".into(),
        "--set".into(),
        "mica_texture_skin_texture=builtin:shingles".into(),
    ]);
    same(
        &off,
        &cli_gcode_with(&paths, &args, features.clone()),
        "textures off",
    );
    let saved = temp("textures-off.3mf");
    app.save_project_to(&saved, Flavour::Prusa).unwrap();
    let mut reopened = gui(&ctx, &CORE_ONE).unwrap();
    reopened.prefs.features = features;
    open(&mut reopened, &[saved]);
    assert_eq!(
        reopened
            .effective_config()
            .and_then(|c| c.raw("mica_texture_skin"))
            .as_deref(),
        Some("external"),
        "the texture stays in the project"
    );
    assert_eq!(reopened.features_off_in_use(), vec![Feature::Textures]);
}

/// Brick layers (a hidden opt-in): turned on by hand, the GUI and the CLI slice alike, and a
/// saved project keeps the settings; without the opt-in both slice without it and the app
/// says so.
fn brick_layers_case(printer: &Printer, flavour: Flavour) {
    use crate::state::Feature;
    let ctx = egui::Context::default();
    let Some(mut app) = gui(&ctx, printer) else {
        return;
    };
    let unlocked = crate::state::Features {
        brick_layers: true,
        ..Default::default()
    };
    app.prefs.features = unlocked.clone();
    let paths = [models().join("hole_block.stl")];
    open(&mut app, &paths);
    let plain = gui_gcode(&app, 0);
    let settings = [
        ("mica_brick_layers", "1"),
        ("mica_brick_layers_flow_ratio", "110%"),
    ];
    for (key, raw) in settings {
        app.apply_edit(
            Some(crate::overlays::Target::Print),
            crate::panels::editor::Edit::Set {
                key: key.into(),
                raw: raw.into(),
            },
        );
    }
    let mut args = printer.args(true);
    for (k, v) in settings {
        args.extend(["--set".into(), format!("{k}={v}")]);
    }
    let what = format!("brick layers on {}", printer.printer);
    let bricked = gui_gcode(&app, 0);
    assert_ne!(bricked, plain, "{what}: walls raised");
    same(
        &bricked,
        &cli_gcode_with(&paths, &args, unlocked.clone()),
        &what,
    );
    let file = temp(&format!("bricked-{flavour:?}.3mf"));
    app.save_project_to(&file, flavour).unwrap();
    // The saved project slices alike, reopened in the app and with the CLI.
    let cli_saved = cli_gcode_with(
        std::slice::from_ref(&file),
        &printer.args(false),
        unlocked.clone(),
    );
    let mut reopened = gui(&ctx, printer).unwrap();
    reopened.prefs.features = unlocked;
    open(&mut reopened, std::slice::from_ref(&file));
    let saved = gui_gcode(&reopened, 0);
    assert_ne!(saved, plain, "{what}: the project keeps brick layers");
    same(&saved, &cli_saved, &format!("{what}, saved as {flavour:?}"));
    // Without the opt-in: plain G-code in both, and a notice.
    app.prefs.features = crate::state::Features::default();
    app.changed();
    assert_eq!(app.features_off_in_use(), vec![Feature::BrickLayers]);
    let off = gui_gcode(&app, 0);
    same(&off, &plain, &format!("{what}, not unlocked"));
    same(
        &off,
        &cli_gcode_with(&paths, &args, crate::state::Features::default()),
        &format!("{what}, not unlocked, CLI"),
    );
}

#[test]
fn brick_layers_on_the_core_one() {
    brick_layers_case(&CORE_ONE, Flavour::Prusa);
}

#[test]
fn brick_layers_on_the_u1() {
    brick_layers_case(&U1, Flavour::Orca);
}

/// Profiles imported from OrcaSlicer and PrusaSlicer slice the same in the GUI and with
/// `mica slice --print/--filament <their names>`: both read Mica's profile store, and an
/// imported Orca preset composes the same way on both sides (`user::orca_config`). The fake
/// slicer folders live in a temporary home, never the user's.
#[test]
fn imported_profiles_slice_alike() {
    let tag = NEXT.fetch_add(1, Ordering::Relaxed);
    let (fine, pla) = (format!("Fine {tag}"), format!("PLA Tweaked {tag}"));
    // A filament on Orca's shared library: listed for (and composed on) every Orca printer.
    let lib = format!("Lib PLA {tag}");
    let home = temp("home").parent().unwrap().to_path_buf();
    let write = |path: PathBuf, text: String| {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write(
        home.join(format!(
            "Library/Application Support/OrcaSlicer/user/default/process/{fine}.json"
        )),
        format!(
            r#"{{"from":"User","inherits":"{}","name":"{fine}","sparse_infill_density":"10%","wall_loops":"3"}}"#,
            U1.print
        ),
    );
    write(
        home.join(format!(
            "Library/Application Support/OrcaSlicer/user/default/filament/{lib}.json"
        )),
        format!(
            r#"{{"from":"User","inherits":"Generic PLA @System","name":"{lib}","nozzle_temperature":["212"]}}"#
        ),
    );
    write(
        home.join(format!(
            "Library/Application Support/PrusaSlicer/filament/{pla}.ini"
        )),
        format!(
            "inherits = {}\ntemperature = 212\nfirst_layer_temperature = 214\n",
            CORE_ONE.filament
        ),
    );
    // The store the CLI reads in this (isolated) process.
    let root = Store::default_root();
    let ctx = egui::Context::default();
    let create = || {
        App::create(
            &ctx,
            Options {
                prefs_path: None,
                store: Some(Store::open(&root)),
                files: vec![],
                headless: true,
            },
        )
    };
    if !mica_testkit::require_orca_profiles() {
        return;
    }
    let mut app = create();
    assert!(
        app.profiles.vendor_index("Snapmaker").is_some(),
        "no Snapmaker profiles"
    );
    app.prefs.add_printer("Snapmaker", U1.printer);
    app.profiles.look_in(&home, None);
    let (rows, errors) = app.profiles.import_candidates(&app.prefs);
    assert!(errors.is_empty(), "{errors:?}");
    let chosen: Vec<_> = rows
        .iter()
        .filter(|r| r.found.name == fine || r.found.name == pla || r.found.name == lib)
        .collect();
    assert_eq!(chosen.len(), 3);
    let done = app.profiles.import(&chosen, &mut app.prefs);
    assert!(done.errors.is_empty(), "{:?}", done.errors);
    let cases = [
        Printer {
            print: Box::leak(fine.clone().into_boxed_str()),
            ..U1
        },
        Printer {
            filament: Box::leak(pla.clone().into_boxed_str()),
            ..CORE_ONE
        },
        Printer {
            filament: Box::leak(lib.clone().into_boxed_str()),
            ..U1
        },
    ];
    for printer in &cases {
        let mut app = create();
        let (vi, _, name) = app
            .profiles
            .all_printers()
            .into_iter()
            .find(|(_, _, n)| n == printer.printer)
            .unwrap();
        app.profiles.select_printer(vi, &name);
        app.profiles.process = printer.print.into();
        app.profiles.refresh();
        app.profiles.filaments.fill(printer.filament.into());
        if let Some(bed) = printer.bed {
            app.profiles.bed = bed.into();
        }
        app.profiles.refresh();
        app.settings_changed();
        assert_eq!(app.profiles.process, printer.print);
        assert_eq!(app.profiles.filaments[0], printer.filament);
        // The imported values took effect.
        let config = app.config.as_ref().unwrap();
        if printer.print == fine {
            assert_eq!(config.float("fill_density"), Some(10.));
        } else {
            assert_eq!(config.float_at("temperature", 0), Some(212.));
        }
        let paths = [models().join("cube20.stl")];
        open(&mut app, &paths);
        same(
            &gui_gcode(&app, 0),
            &cli_gcode(&paths, &printer.args(true)),
            &format!("imported profiles on {}", printer.printer),
        );
    }
    // Leave the shared store as it was for the other tests of this process.
    for (kind, name) in &done.names {
        let _ = app.profiles.store.delete(*kind, name);
    }
}

/// Export all plates and `mica slice --each-plate -o DIR` of a multi-plate project write the
/// same files: the same names, and per plate the same G-code (text compared as the other
/// cases; binary byte for byte under `cargo xtask parity`'s fixed clock).
fn each_plate_case(printer: &Printer, presets: bool, edits: &[(&str, &str)]) {
    let ctx = egui::Context::default();
    let Some(mut app) = gui(&ctx, printer) else {
        return;
    };
    let path = models().join("plates-orca.3mf");
    open(&mut app, std::slice::from_ref(&path));
    assert!(app.plates.len() > 1, "a multi-plate project");
    // As `project_case`: instances the project assigns to no plate stay out (the CLI leaves
    // them out of every plate).
    let bed = app.bed.clone();
    for plate in &mut app.plates.plates {
        plate.objects.retain(|o| {
            let (min, max) = o.bounds();
            min.x < bed.max.x && max.x > bed.min.x && min.y < bed.max.y && max.y > bed.min.y
        });
    }
    let mut args = printer.args(presets);
    for (k, v) in edits {
        app.overlays.print.insert(k.to_string(), v.to_string());
        args.extend(["--set".into(), format!("{k}={v}")]);
    }
    app.settings_changed();
    let gui_dir = temp("gui");
    std::fs::create_dir_all(&gui_dir).unwrap();
    app.export_all_to(gui_dir.clone(), &ctx);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
    while app.export_all.as_ref().is_some_and(|e| e.stage.busy())
        && std::time::Instant::now() < deadline
    {
        app.poll(&ctx);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    match &app.export_all.as_ref().unwrap().stage {
        crate::plate_batch::ExportStage::Done { skipped, .. } => {
            assert!(skipped.is_empty(), "{skipped:?}")
        }
        _ => panic!("Export all plates did not finish: {}", app.status),
    }
    let cli_dir = temp("cli");
    let mut all = vec!["slice".to_string(), path.to_string_lossy().into_owned()];
    all.extend(args);
    all.extend([
        "--each-plate".into(),
        "-o".into(),
        cli_dir.to_string_lossy().into_owned(),
    ]);
    mica_cli::run(&all).unwrap_or_else(|e| panic!("mica {all:?}: {e}"));
    let list = |dir: &Path| {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    };
    let names = list(&gui_dir);
    assert_eq!(
        names.len(),
        app.plates.len(),
        "one file per plate: {names:?}"
    );
    assert_eq!(names, list(&cli_dir), "the same file names");
    let fixed_clock = std::env::var("SOURCE_DATE_EPOCH").as_deref() == Ok("0");
    for name in &names {
        let (gui, cli) = (
            std::fs::read(gui_dir.join(name)).unwrap(),
            std::fs::read(cli_dir.join(name)).unwrap(),
        );
        if name.ends_with(".gcode") {
            same(&gui, &cli, &format!("{name} on {}", printer.printer));
        } else if fixed_clock {
            assert!(gui == cli, "{name}: GUI and CLI binary G-code differ");
        }
    }
}

#[test]
fn each_plate_matches_export_all_plates_on_the_u1() {
    each_plate_case(&U1, false, &[("wipe_tower", "1")]);
}

#[test]
fn each_plate_matches_export_all_plates_on_the_xl() {
    each_plate_case(&XL, true, &[]);
}
