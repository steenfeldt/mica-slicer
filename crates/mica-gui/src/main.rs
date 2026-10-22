// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

// Optimized Windows builds are GUI programs (no console window opens with Mica); debug builds
// keep the console for their output.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(test)]
mod add_printer_tests;
mod app;
mod brick_layers;
mod brim_ears;
mod builtin;
mod calibration;
mod checks;
#[cfg(test)]
mod cli_parity_tests;
mod commands;
mod credits;
mod cut;
#[cfg(test)]
mod cut_ui_tests;
mod emboss;
mod gcode_doc;
mod import;
#[cfg(test)]
mod import_tests;
mod intents;
mod keymap;
mod layer_edit;
mod library;
mod library_drop;
#[cfg(test)]
mod library_drop_tests;
mod library_preview;
mod mcp_live;
// Live mode needs a Unix socket (not on Windows yet).
#[cfg(test)]
mod first_printer_tests;
#[cfg(all(test, unix))]
mod mcp_live_tests;
mod measure;
mod menu;
mod mesh_tools;
mod mixes;
mod monkey;
#[cfg(test)]
mod move_drag_tests;
mod notifications;
#[cfg(test)]
mod objects_ui_tests;
mod overlays;
mod paint;
mod panels;
mod plate_batch;
#[cfg(test)]
mod plate_batch_tests;
mod plate_menu;
mod plate_settings;
mod plates;
#[cfg(test)]
mod plates_ui_tests;
mod preset_names;
mod preview;
#[cfg(test)]
mod printer_filament_tests;
mod printhost;
mod project;
#[cfg(test)]
mod project_state_tests;
mod prusa_download;
#[cfg(test)]
mod regressions;
mod render;
mod save;
mod scene;
mod screenshot;
mod settings;
mod slice_cache;
#[cfg(test)]
mod slice_error_tests;
mod state;
mod textures;
mod theme;
mod tour;
#[cfg(test)]
mod tour_tests;
mod tower;
mod tree;
mod updater;
mod viewport;
#[cfg(test)]
mod welcome_tests;
mod worker;

const USAGE: &str = "Mica Slicer
  mica-gui [--welcome] [--whats-new] [model.stl|project.3mf …]
  mica-gui --list-printers   where the vendor profiles come from, and the printers loaded at start
  mica-gui --screenshot out.png model.stl|project.3mf [--preview] [--xl] [--printer name] [--select N] [--plate N]
           [--gizmo move|rotate|scale] [--focus] [--view iso|top|bottom|below|front|rear|left|right]
           [--pointer X,Y] [--plates N] [--tab Quality] [--advanced] [--search text]
           [--calibrate TEST[:TOOL]] [--calibrate-create] [--calibrate-result V] [--calibrate-save]
           [--object-mode] [--object-set key=value] [--plate-scope] [--plate-set key=value]
           [--set key=value] [--printer-editor]
           [--filament-editor N] [--all-printers] [--light] [--size WxH] [--scale S] [--crop W]
           [--tower X,Y[,R]] [--select-tower] [--command tower-free|tower-disable|tower-settings]
           [--preferences] [--auto-slice] [--theme dark|light|follow-system] [--new-project]
           [--welcome] [--whats-new] [--tour N]
           [--library] [--library-list] [--library-search T] [--library-select N] [--library-filter F]
           [--library-group G] [--library-project NAME]
  mica-gui --screenshot-3d out.png model.stl [--preview] [--xl] [--color tool|feature|speed|fan|width|height|layertime]
           [--view iso|top|…] [--select N] [--gizmo move|rotate|scale] [--plates N] [--light] [--ortho] [--layer N] [--size WxH]
           [--printer NAME] [--first-plate] [--bench DIR [--tag T] [--frames N] [--only orbit,hover,shots]]
  mica-gui --monkey [--quick] [--seeds N] [--first S] [--steps N] [--jobs J] [--seed S] [--verbose] [--save-at STEP]";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    // Textured meshes (textures on all surfaces) are kept in the settings folder's cache.
    mica_core::texture::displace::set_disk_cache(Some(
        mica_core::user_data::config_dir()
            .join("cache")
            .join("textures"),
    ));
    if args.iter().any(|a| a == "--list-printers") {
        list_printers();
        return Ok(());
    }
    if args.iter().any(|a| a == "--monkey") {
        return monkey::run(&args).map_err(Into::into);
    }
    if args.iter().any(|a| a == "--screenshot") {
        return screenshot::run(&args).map_err(Into::into);
    }
    if args.iter().any(|a| a == "--screenshot-3d") {
        let args: Vec<String> = args
            .iter()
            .map(|a| {
                if a == "--screenshot-3d" {
                    "--screenshot".to_string()
                } else {
                    a.clone()
                }
            })
            .collect();
        return render::screenshot::run(&args).map_err(Into::into);
    }
    // The Welcome window / this version's What's new at launch, whatever mica.toml remembers.
    let launch = panels::welcome::Launch {
        welcome: args.iter().any(|a| a == "--welcome"),
        whats_new: args.iter().any(|a| a == "--whats-new"),
    };
    let args: Vec<String> = args
        .into_iter()
        .filter(|a| a != "--welcome" && a != "--whats-new")
        .collect();
    if args
        .iter()
        .any(|a| a == "--help" || a == "-h" || a.starts_with('-'))
    {
        println!("{USAGE}");
        return Ok(());
    }
    let files: Vec<std::path::PathBuf> = args.iter().map(std::path::PathBuf::from).collect();
    // A window process: an update must not swap the app from under it (see mica_update::instance).
    if let Ok(exe) = std::env::current_exe() {
        mica_update::instance::hold(&exe);
    }
    let (prefs, _) = state::Prefs::load(&state::Prefs::default_path());
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([
                prefs.window.width.clamp(900., 6000.),
                prefs.window.height.clamp(600., 4000.),
            ])
            .with_min_inner_size([900., 600.])
            .with_title("Mica Slicer")
            .with_icon(app_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "Mica Slicer",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, files, launch)))),
    )?;
    Ok(())
}

/// `mica-gui --list-printers`: where this build finds its vendor profiles and the printers it
/// loads at start (checks a package without opening a window; reads only).
fn list_printers() {
    use mica_profiles::vendor_profiles::prusa;
    let profiles = panels::profiles::Profiles::discover_with_store(
        mica_profiles::user::Store::open(mica_profiles::user::Store::default_root()),
    );
    match prusa::find_bundle(None) {
        Some(p) => println!("Prusa profiles: {}", p.display()),
        None => println!(
            "Prusa profiles: not downloaded yet. {}",
            prusa::DOWNLOAD_NOTE
        ),
    }
    match &profiles.orca_root {
        Some(root) => println!(
            "Orca profiles: {} ({})",
            root.display(),
            mica_profiles::orca::discovery::vendor_indexes(root).join(", ")
        ),
        None => println!("Orca profiles: none found"),
    }
    for (_, vendor, name) in profiles.all_printers() {
        println!("{vendor}/{name}");
    }
    for w in &profiles.warnings {
        eprintln!("warning: {w}");
    }
}

/// The window and Dock icon (assets/icon/README.md). The art is compiled in, so decoding can
/// only fail if the file in the repo is broken; then the window keeps the platform default
/// and the reason goes to stderr.
fn app_icon() -> eframe::egui::IconData {
    const PNG: &[u8] = include_bytes!("../assets/icon/mica-512.png");
    match image::load_from_memory(PNG) {
        Ok(img) => {
            let rgba = img.into_rgba8();
            let (width, height) = rgba.dimensions();
            eframe::egui::IconData {
                rgba: rgba.into_raw(),
                width,
                height,
            }
        }
        Err(e) => {
            eprintln!("mica: app icon could not be decoded: {e}");
            eframe::egui::IconData::default()
        }
    }
}
