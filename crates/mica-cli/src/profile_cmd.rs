// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! `mica profile …`: Mica's own user profiles.

use std::path::{Path, PathBuf};

use mica_profiles::import::{self, Source, SourceDir};
use mica_profiles::prusa::{Bundle, Kind};
use mica_profiles::user::{Store, UserProfile, export_prusa_user_preset, kind_name};

pub const USAGE: &str = "\
  mica profile list                         list your Mica profiles
  mica profile show <name>                  show a profile's overrides
  mica profile import prusaslicer [<dir>]   import PrusaSlicer user presets (default: its settings folder)
  mica profile export prusaslicer <kind> <name> <file.ini>
                                            write a profile as a PrusaSlicer user preset
  mica profile delete <kind> <name>         move a profile to the trash folder";

const KINDS: [Kind; 3] = [Kind::Printer, Kind::Print, Kind::Filament];

fn prusaslicer_datadir() -> Option<PathBuf> {
    let home = mica_core::user_data::home()?;
    [
        home.join("Library/Application Support/PrusaSlicer"),
        home.join(".config/PrusaSlicer"),
        home.join(".var/app/com.prusa3d.PrusaSlicer/config/PrusaSlicer"),
    ]
    .into_iter()
    .chain(mica_core::user_data::var_dir("APPDATA").map(|a| a.join("PrusaSlicer")))
    .find(|p| p.is_dir())
}

fn parse_kind(s: &str) -> Result<Kind, String> {
    match s {
        "printer" => Ok(Kind::Printer),
        "print" => Ok(Kind::Print),
        "filament" => Ok(Kind::Filament),
        _ => Err(format!(
            "unknown profile kind '{s}' (printer, print or filament)"
        )),
    }
}

pub fn run(
    args: &[String],
    bundle: impl FnOnce() -> Result<(Bundle, PathBuf), String>,
) -> Result<(), String> {
    let store = Store::open(Store::default_root());
    match args.first().map(String::as_str) {
        Some("list") | None => {
            println!("# {}", store.root.display());
            for kind in KINDS {
                for p in store.list(kind).map_err(|e| e.to_string())? {
                    println!(
                        "{:<9} {}  (inherits {}, {} override(s))",
                        kind_name(kind),
                        p.name,
                        p.inherits,
                        p.settings.len()
                    );
                }
            }
            Ok(())
        }
        Some("show") => {
            let name = args.get(1).ok_or("profile show needs a name")?;
            let p = KINDS
                .iter()
                .find_map(|&k| store.load(k, name).ok())
                .ok_or_else(|| format!("no profile named \"{name}\""))?;
            print!("{}", p.to_toml());
            Ok(())
        }
        Some("delete") => {
            let kind = parse_kind(args.get(1).ok_or("profile delete needs a kind")?)?;
            let name = args.get(2).ok_or("profile delete needs a name")?;
            let to = store.delete(kind, name).map_err(|e| e.to_string())?;
            eprintln!("moved to {}", to.display());
            Ok(())
        }
        Some("export") => match args.get(1).map(String::as_str) {
            Some("prusaslicer") => {
                let kind = parse_kind(args.get(2).ok_or("profile export needs a kind")?)?;
                let name = args.get(3).ok_or("profile export needs a name")?;
                let file = args.get(4).ok_or("profile export needs an output file")?;
                let profile = store.load(kind, name).map_err(|e| e.to_string())?;
                let (bundle, _) = bundle()?;
                let text =
                    export_prusa_user_preset(&profile, &bundle).map_err(|e| e.to_string())?;
                std::fs::write(file, text).map_err(|e| format!("{file}: {e}"))?;
                eprintln!("wrote {file}");
                Ok(())
            }
            other => Err(format!(
                "unknown export target {other:?} (supported: prusaslicer)"
            )),
        },
        Some("import") => match args.get(1).map(String::as_str) {
            Some("prusaslicer") => {
                let dir = match args.get(2) {
                    Some(d) => PathBuf::from(d),
                    None => prusaslicer_datadir()
                        .ok_or("PrusaSlicer settings folder not found; pass it explicitly")?,
                };
                let (bundle, _) = bundle()?;
                import_prusaslicer(&dir, &bundle, &store)
            }
            other => Err(format!(
                "unknown import source {other:?} (supported: prusaslicer)"
            )),
        },
        Some(other) => Err(format!("unknown profile command '{other}'")),
    }
}

/// Imports every PrusaSlicer user preset in `dir`. Importing again updates Mica's copies
/// (same file, same profile); a name Mica already uses gets " (PrusaSlicer)".
fn import_prusaslicer(dir: &Path, bundle: &Bundle, store: &Store) -> Result<(), String> {
    let mut imported = 0;
    let (found, errors) = import::scan(&[SourceDir {
        source: Source::PrusaSlicer,
        root: dir.to_path_buf(),
    }]);
    for e in errors {
        eprintln!("skipped {e}");
    }
    let mut existing = store.list_all().map_err(|e| e.to_string())?;
    let now = import::now();
    for f in found {
        let name = import::target_name(&f, &existing, |_| false);
        match import::prusa_profile(&f, bundle, &name, now) {
            Ok(profile) => {
                let path = import::save(store, profile.clone(), &mut existing)
                    .map_err(|e| e.to_string())?;
                report(&profile, &path);
                imported += 1;
            }
            Err(e) => eprintln!("skipped {}: {e}", f.path.display()),
        }
    }
    eprintln!(
        "{imported} profile(s) imported into {}",
        store.root.display()
    );
    Ok(())
}

fn report(p: &UserProfile, path: &Path) {
    eprintln!(
        "{} \"{}\" (inherits \"{}\"): {} override(s) → {}",
        kind_name(p.kind),
        p.name,
        p.inherits,
        p.settings.len(),
        path.display()
    );
    for (k, v) in &p.settings {
        let shown = if v.len() > 60 {
            format!(
                "{}…",
                &v[..v.char_indices().nth(60).map_or(v.len(), |(i, _)| i)]
            )
        } else {
            v.clone()
        };
        eprintln!("    {k} = {shown}");
    }
}
