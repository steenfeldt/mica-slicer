// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original
// Source (original): Mica CLI project resolution and mica-profiles shared printer matching.
// Changes: Detect the project printer and vendor before composing profiles; preserve explicit flags.
// Changes: Slice with the project's whole embedded configuration; `update` keeps the old preset-plus-listed-changes mode.
// Changes: A default process or filament that does not suit the printer gives way to a compatible one, with a warning.
// Changes: Mica user profiles built on an Orca vendor (imported Orca presets and Mica's own) compose as in the GUI.
// Changes: Filaments built on Orca's shared filament library compose for every Orca vendor, as in the GUI.

//! Resolve project presets and the project's own values before importing objects or
//! preparing paths. A project slices with the values it was saved with (as OrcaSlicer and
//! PrusaSlicer do): the presets it names are composed and its embedded configuration goes on
//! top. Explicit `--print`, `--filament` and `--bed` naming other presets than the project's
//! replace the project's values for that scope; `--set` applies last.
use super::{ProjectMode, SliceArgs, load_bundle};
use mica_core::{config::Config, threemf};
use mica_profiles::{
    orca,
    project::{self, Decision, Mode, ProjectSettings},
    prusa,
};

pub(super) struct Resolved {
    pub config: Config,
    /// Present only when the resolution decision used the embedded settings.
    pub original: Option<threemf::RawConfig>,
    pub selected_orca: Option<orca::RawConfig>,
}

pub(super) fn resolve(a: &SliceArgs) -> Result<Resolved, String> {
    let mut raw: Option<threemf::RawConfig> = None;
    // Mica's own options an Orca project keeps in Mica's sidecar.
    let mut mica = threemf::Settings::new();
    for path in std::iter::once(&a.model).chain(&a.extra_models) {
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("3mf"))
        {
            continue;
        }
        if let Some(c) =
            threemf::load_config(path).map_err(|e| format!("{}: {e}", path.display()))?
        {
            if a.project_settings != Some(ProjectMode::Ignore)
                && raw
                    .as_ref()
                    .is_some_and(|old| old.path != c.path || old.text != c.text)
            {
                return Err("input projects have conflicting embedded settings; slice separately or use --project-settings ignore".into());
            }
            raw = Some(c);
            mica = threemf::load_mica_settings(path)
                .map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    let settings = raw.as_ref().and_then(ProjectSettings::parse).map(|mut s| {
        s.mica = mica.clone();
        s
    });
    if raw.is_some() && settings.is_none() && a.project_settings != Some(ProjectMode::Ignore) {
        return Err("could not parse embedded project settings; use --project-settings ignore to select profiles only".into());
    }
    let mut selected = a.clone();
    if let Some(settings) = settings.as_ref() {
        select_printer(&mut selected, settings);
    }
    let a = &selected;
    if a.vendor == "Prusa" && (a.profiles_dir.is_some() || a.bed.is_some()) {
        return Err("--profiles-dir and --bed require an Orca --vendor".into());
    }
    if a.vendor == "Prusa" {
        let (bundle, _) = load_bundle(a.bundle.as_deref())?;
        let printers = names(bundle.preset_names(prusa::Kind::Printer));
        let settings = enabled(a, settings.as_ref(), printers);
        let processes = names(bundle.compatible_presets(prusa::Kind::Print, &a.printer, None));
        let process = choose(a, settings, &processes);
        let materials =
            names(bundle.compatible_presets(prusa::Kind::Filament, &a.printer, Some(&process)));
        let mut config = compose(a, settings, &process, &materials, None, |filament| {
            bundle
                .full_config(&a.printer, &process, filament)
                .map_err(|e| e.to_string())
        })?;
        if let Some(source) = raw.as_ref().and_then(ProjectSettings::parse) {
            source.apply_colours(&mut config)?;
        }
        Ok(Resolved {
            config,
            original: settings.and(raw.clone()),
            selected_orca: None,
        })
    } else {
        let (mut bundle, _) = orca::discovery::load_installed(a.profiles_dir.as_deref(), &a.vendor)
            .map_err(|e| e.to_string())?;
        // Mica's user profiles on this vendor, as the GUI lists them: imported Orca presets
        // registered as user presets, Mica's own as overlays on their parent.
        let user = user_profiles(&a.vendor, &bundle);
        for w in mica_profiles::user::register_orca(&mut bundle, &user) {
            eprintln!("warning: {w}");
        }
        let find =
            |kind: prusa::Kind, name: &str| user.iter().find(|p| p.kind == kind && p.name == name);
        let base = |kind: prusa::Kind, name: &str| {
            find(kind, name).map_or(name.to_string(), |p| p.orca_base().to_string())
        };
        // Mica's own profiles (no Orca preset of their own) follow their parent's compatibility.
        let with_user = |kind: prusa::Kind, mut list: Vec<String>| {
            let extra: Vec<String> = user
                .iter()
                .filter(|p| p.kind == kind && p.orca.is_none() && list.contains(&p.inherits))
                .map(|p| p.name.clone())
                .collect();
            list.extend(extra);
            list
        };
        let printer_base = base(prusa::Kind::Printer, &a.printer);
        let printers = names(bundle.preset_names(orca::Kind::Machine));
        let settings = enabled(a, settings.as_ref(), printers);
        let processes = with_user(
            prusa::Kind::Print,
            names(bundle.compatible_presets(orca::Kind::Process, &printer_base, None)),
        );
        let process = choose(a, settings, &processes);
        let materials = with_user(
            prusa::Kind::Filament,
            names(bundle.compatible_presets(
                orca::Kind::Filament,
                &printer_base,
                Some(&base(prusa::Kind::Print, &process)),
            )),
        );
        // The presets are composed for the project's plate type unless --bed names one.
        let bed = a
            .bed
            .clone()
            .or_else(|| settings.and_then(|s| s.bed.clone()));
        let mut config = compose(
            a,
            settings,
            &process,
            &materials,
            Some(bundle.dialect),
            |filament| {
                mica_profiles::user::orca_config(
                    &bundle,
                    (&a.printer, find(prusa::Kind::Printer, &a.printer)),
                    (&process, find(prusa::Kind::Print, &process)),
                    (filament, find(prusa::Kind::Filament, filament)),
                    bed.as_deref(),
                )
            },
        )?;
        let selected_orca = if a.export_3mf.is_some()
            && (settings.is_none()
                || raw
                    .as_ref()
                    .is_some_and(|c| !c.path.ends_with("project_settings.config")))
        {
            let tools = config.vector_len("nozzle_diameter").max(1);
            let filaments: Vec<&str> = (0..tools)
                .map(|t| {
                    config
                        .string_at("filament_settings_id", t)
                        .unwrap_or(&a.filament)
                })
                .collect();
            let selected = project::selected_orca(
                &bundle,
                &a.printer,
                &process,
                &filaments,
                config.string("curr_bed_type").unwrap_or_default(),
                &colours(&config),
            )?;
            Some(selected)
        } else {
            None
        };
        if let Some(source) = raw.as_ref().and_then(ProjectSettings::parse) {
            source.apply_colours(&mut config)?;
        }
        Ok(Resolved {
            config,
            original: settings.and(raw.clone()),
            selected_orca,
        })
    }
}

/// The Mica user profiles built on Orca vendor `vendor` (Mica's settings folder), and the
/// filaments built on Orca's shared library, which every Orca vendor lists.
fn user_profiles(vendor: &str, bundle: &orca::Bundle) -> Vec<mica_profiles::user::UserProfile> {
    let store = mica_profiles::user::Store::open(mica_profiles::user::Store::default_root());
    [
        prusa::Kind::Printer,
        prusa::Kind::Print,
        prusa::Kind::Filament,
    ]
    .into_iter()
    .flat_map(|k| match store.list(k) {
        Ok(list) => list,
        Err(e) => {
            eprintln!("warning: {e}");
            vec![]
        }
    })
    .filter(|p| mica_profiles::user::on_orca_vendor(p, vendor, bundle))
    .collect()
}

/// Consider the same vendor bundles as the GUI, or only the explicitly selected vendor.
/// Missing bundles cannot match; normal resolution still reports errors for selected profiles.
fn select_printer(a: &mut SliceArgs, settings: &ProjectSettings) {
    if a.printer_explicit || a.project_settings == Some(ProjectMode::Ignore) {
        return;
    }
    let vendors = if a.vendor_explicit {
        vec![a.vendor.clone()]
    } else if a.bundle.is_some() {
        vec!["Prusa".into()]
    } else {
        vec!["Prusa".into(), "Snapmaker".into()]
    };
    let mut printers = Vec::new();
    for (vi, vendor) in vendors.iter().enumerate() {
        let names = if vendor == "Prusa" {
            load_bundle(a.bundle.as_deref())
                .ok()
                .map(|(b, _)| self::names(b.preset_names(prusa::Kind::Printer)))
        } else {
            orca::discovery::load_installed(a.profiles_dir.as_deref(), vendor)
                .ok()
                .map(|(b, _)| self::names(b.preset_names(orca::Kind::Machine)))
        };
        printers.extend(names.into_iter().flatten().map(|n| (vi, vendor.clone(), n)));
    }
    if let Some((vi, printer)) = project::match_printer(&printers, settings) {
        a.vendor.clone_from(&vendors[vi]);
        a.vendor_defaults();
        a.printer = printer;
    }
}

pub(super) fn colours(config: &Config) -> Vec<String> {
    (0..config.vector_len("nozzle_diameter").max(1))
        .map(|t| {
            config
                .string_at("filament_colour", t)
                .unwrap_or("#FFFFFF")
                .to_string()
        })
        .collect()
}

fn names(items: Vec<&str>) -> Vec<String> {
    items.into_iter().map(str::to_string).collect()
}
fn enabled<'a>(
    a: &SliceArgs,
    settings: Option<&'a ProjectSettings>,
    printers: Vec<String>,
) -> Option<&'a ProjectSettings> {
    let settings = settings?;
    let printers = printers
        .into_iter()
        .map(|p| (0, a.vendor.clone(), p))
        .collect::<Vec<_>>();
    let matched = project::match_printer(&printers, settings);
    let matches = project::decide(matched.as_ref(), 0, &a.printer) == Decision::Use;
    let use_settings = a
        .project_settings
        .map_or(matches, |m| m != ProjectMode::Ignore);
    if use_settings {
        eprintln!(
            "3MF note: using embedded project settings with selected printer {}{}",
            a.printer,
            if a.project_settings == Some(ProjectMode::Update) {
                " (updated to today's presets, keeping the changes the project lists)"
            } else {
                ""
            }
        );
        Some(settings)
    } else {
        eprintln!(
            "3MF note: ignoring embedded project settings ({})",
            if a.project_settings == Some(ProjectMode::Ignore) {
                "--project-settings ignore"
            } else {
                "project printer does not match the selected printer; override with --project-settings use"
            }
        );
        None
    }
}

/// The process preset: an explicit `--print` wins, else the project's (by name), else the
/// default (one for the selected printer, [`fallback`]).
fn choose(a: &SliceArgs, settings: Option<&ProjectSettings>, candidates: &[String]) -> String {
    if a.print_explicit {
        return a.print.clone();
    }
    let wanted = settings.and_then(|s| s.process.as_deref());
    if let Some(found) = wanted.and_then(|w| project::match_name(candidates, w)) {
        return found;
    }
    let kept = fallback("process", &a.print, candidates, None, &a.printer);
    if let Some(wanted) = wanted {
        eprintln!("3MF note: process {wanted} not found, kept {kept}");
    }
    kept
}

/// The default preset `default` when it suits the selected printer (it is among the compatible
/// `candidates`), else, with a warning, the same preset for this printer (its name before
/// ` @`), a generic one of the project's `material` type, or the first compatible one. The
/// defaults are the CORE One's: a project for another printer whose own presets are not
/// installed must not slice with the CORE One's (or fail as incompatible).
fn fallback(
    what: &str,
    default: &str,
    candidates: &[String],
    material: Option<&str>,
    printer: &str,
) -> String {
    if candidates.is_empty() || candidates.iter().any(|c| c == default) {
        return default.to_string();
    }
    let chosen = project::match_filament(candidates, default, material)
        .unwrap_or_else(|| candidates[0].clone());
    eprintln!(
        "warning: the default {what} {default} does not suit {printer}; using {chosen} (pass --{} to choose)",
        if what == "process" { "print" } else { what }
    );
    chosen
}

/// Bed temperatures follow the plate type: another bed than the project's keeps the presets'.
const BED_KEYS: &[&str] = &[
    "bed_temperature",
    "first_layer_bed_temperature",
    "curr_bed_type",
];

fn compose(
    a: &SliceArgs,
    settings: Option<&ProjectSettings>,
    process: &str,
    materials: &[String],
    dialect: Option<orca::Dialect>,
    resolve: impl Fn(&str) -> Result<Config, String>,
) -> Result<Config, String> {
    let default_filament = |material: Option<&str>| {
        if a.filament_explicit {
            a.filament.clone()
        } else {
            // The defaults are PLA: another PLA unless the project names its material.
            let material = material.or(Some("PLA"));
            fallback("filament", &a.filament, materials, material, &a.printer)
        }
    };
    let Some(settings) = settings else {
        return resolve(&default_filament(None));
    };
    // The project's filament of `tool`, matched to an installed preset.
    let project_filament = |tool: usize| {
        settings.filaments.get(tool).and_then(|wanted| {
            project::match_filament(
                materials,
                wanted,
                settings.filament_types.get(tool).map(String::as_str),
            )
        })
    };
    let filament = |tool: usize| {
        if a.filament_explicit {
            return a.filament.clone();
        }
        project_filament(tool).unwrap_or_else(|| {
            let material = settings.filament_types.get(tool).map(String::as_str);
            let kept = default_filament(material);
            if let Some(wanted) = settings.filaments.get(tool) {
                eprintln!(
                    "3MF note: filament {wanted} not found, kept {kept} for T{} (with the project's filament values)",
                    tool + 1
                );
            }
            kept
        })
    };
    let first = filament(0);
    let mut config = resolve(&first)?;
    let mut filaments = vec![first];
    let tools = config.vector_len("nozzle_diameter").max(1);
    for tool in 1..tools {
        let name = filament(tool);
        project::merge_filament(&mut config, &resolve(&name)?, tool)?;
        filaments.push(name);
    }
    config
        .set_raw(
            "filament_settings_id",
            &mica_core::config::escape_strings(&filaments),
        )
        .map_err(|e| e.to_string())?;
    let mode = if a.project_settings == Some(ProjectMode::Update) {
        Mode::Listed
    } else {
        Mode::Embedded
    };
    let (mut layer, notes) = settings.layer(&config, dialect, mode);
    for note in notes {
        eprintln!("3MF note: {note}");
    }
    // Explicit presets other than the project's replace its values for their scope.
    let project_process = settings
        .process
        .as_deref()
        .map(|p| project::match_name(&[process.to_string()], p).is_some());
    if a.print_explicit && project_process != Some(true) {
        eprintln!("3MF note: --print {process} replaces the project's process values");
        layer.drop_process();
    }
    if a.filament_explicit {
        let replaced: Vec<String> = (0..tools)
            .filter(|&t| {
                t < settings.filaments.len()
                    && project_filament(t).as_deref() != Some(a.filament.as_str())
            })
            .map(|t| {
                layer.drop_tool(t);
                format!("T{}", t + 1)
            })
            .collect();
        if !replaced.is_empty() {
            eprintln!(
                "3MF note: --filament {} replaces the project's filament values for {}",
                a.filament,
                replaced.join(", ")
            );
        }
    }
    if let Some(bed) = &a.bed
        && settings.bed.as_ref().is_some_and(|b| b != bed)
    {
        layer.drop_keys(BED_KEYS);
    }
    let differ = layer.count(&config);
    if differ > 0 {
        eprintln!(
            "3MF note: {differ} project value{} differ{} from the current presets{}",
            if differ == 1 { "" } else { "s" },
            if differ == 1 { "s" } else { "" },
            if mode == Mode::Embedded {
                " (--project-settings update uses today's presets with the project's listed changes)"
            } else {
                ""
            }
        );
    }
    layer.apply(&mut config);
    settings.apply_colours(&mut config)?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Fixture(PathBuf);
    impl Fixture {
        fn directory() -> Self {
            static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "mica-cli-settings-{}-{}",
                std::process::id(),
                N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn new() -> Self {
            let fixture = Self::directory();
            let dir = &fixture.0;
            std::fs::write(dir.join("bundle.ini"), "[printer:Fixture (0.4 nozzle)]\nnozzle_diameter = 0.4,0.4\n[printer:Other (0.4 nozzle)]\nnozzle_diameter = 0.4,0.4\n[print:Default process]\nperimeters = 2\n[print:Project process]\nperimeters = 4\n[filament:Default PLA]\ntemperature = 210\nfilament_density = 1.2\n[filament:Project PLA]\ntemperature = 230\nfilament_density = 1.3\n").unwrap();
            let mut project = threemf::load_3mf(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../tests/models/painted-cube.3mf"),
            )
            .unwrap();
            project.config = Some(threemf::RawConfig {
                path: "Metadata/Slic3r_PE.config".into(),
                text: "; printer_settings_id = Fixture (0.4 nozzle) - Copy\n; print_settings_id = Project process\n; filament_settings_id = \"Default PLA\";\"Project PLA\"\n; temperature = 215,245\n; fill_density = 12%\n; support_material_auto = 0\n; support_material_style = organic\n".into(),
            });
            threemf::write(dir.join("project.3mf"), &project, Flavour::Prusa).unwrap();
            fixture
        }

        fn orca(settings: &str) -> Self {
            use mica_core::{mesh::TriangleMesh, threemf::*};
            let fixture = Self::directory();
            let dir = &fixture.0;
            std::fs::create_dir_all(dir.join("Snapmaker/machine")).unwrap();
            std::fs::create_dir_all(dir.join("Snapmaker/process")).unwrap();
            std::fs::create_dir_all(dir.join("Snapmaker/filament")).unwrap();
            std::fs::write(dir.join("Snapmaker.json"), r#"{
                "machine_list":[
                    {"name":"Snapmaker U1 (0.2 nozzle)","sub_path":"machine/u1-02.json"},
                    {"name":"Snapmaker U1 (0.4 nozzle)","sub_path":"machine/u1-04.json"}],
                "process_list":[{"name":"0.20 Standard @Snapmaker U1 (0.4 nozzle)","sub_path":"process/standard.json"}],
                "filament_list":[{"name":"Generic PLA @System","sub_path":"filament/pla.json"}]
            }"#).unwrap();
            for nozzle in ["0.2", "0.4"] {
                std::fs::write(
                    dir.join(format!(
                        "Snapmaker/machine/u1-{}.json",
                        nozzle.replace('.', "")
                    )),
                    format!(
                        r#"{{
                    "name":"Snapmaker U1 ({nozzle} nozzle)","instantiation":"true",
                    "printer_model":"Snapmaker U1","printer_variant":"{nozzle}","default_bed_type":"High Temp Plate",
                    "nozzle_diameter":["{nozzle}","{nozzle}","{nozzle}","{nozzle}"]
                }}"#
                    ),
                )
                .unwrap();
            }
            std::fs::write(dir.join("Snapmaker/process/standard.json"), r#"{"name":"0.20 Standard @Snapmaker U1 (0.4 nozzle)","instantiation":"true","wall_loops":"2"}"#).unwrap();
            std::fs::write(
                dir.join("Snapmaker/filament/pla.json"),
                r#"{"name":"Generic PLA @System","instantiation":"true","filament_type":["PLA"]}"#,
            )
            .unwrap();
            std::fs::write(dir.join("bundle.ini"), format!("[printer:{}]\nnozzle_diameter = 0.4\n[print:{}]\nperimeters = 2\n[filament:{}]\ntemperature = 210\n", crate::DEFAULT_PRINTER, crate::DEFAULT_PRINT, crate::DEFAULT_FILAMENT)).unwrap();
            let source = ResourceId {
                path: "3D/3dmodel.model".into(),
                id: 1,
            };
            let project = Project {
                objects: vec![Object {
                    name: "Synthetic tetrahedron".into(),
                    source: source.clone(),
                    height_ranges: vec![],
                    layer_height_profile: vec![],
                    brim_points: vec![],
                    parts: vec![Part {
                        name: "Tetrahedron".into(),
                        kind: PartKind::Model,
                        source,
                        mesh: TriangleMesh {
                            vertices: vec![[0., 0., 0.], [2., 0., 0.], [0., 2., 0.], [0., 0., 2.]],
                            faces: vec![[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]],
                        },
                        source_mesh: None,
                        painting: Default::default(),
                        transform: Default::default(),
                        settings: Default::default(),
                        metadata: Default::default(),
                    }],
                    instances: vec![Instance {
                        transform: Default::default(),
                        printable: true,
                        plate: None,
                    }],
                    settings: Settings::from([("extruder".into(), "3".into())]),
                    metadata: Default::default(),
                }],
                config: Some(RawConfig {
                    path: "Metadata/project_settings.config".into(),
                    text: settings.into(),
                }),
                ..Default::default()
            };
            write(dir.join("project.3mf"), &project, Flavour::Orca).unwrap();
            fixture
        }

        fn orca_args(&self, extra: &[&str]) -> SliceArgs {
            let mut args = vec![
                self.0.join("project.3mf").display().to_string(),
                "--profiles-dir".into(),
                self.0.display().to_string(),
            ];
            args.extend(extra.iter().map(|s| s.to_string()));
            crate::parse_slice(&args).unwrap()
        }
        /// The fixture bundle and printer only: the project picks process and filaments.
        fn project_args(&self, extra: &[&str]) -> SliceArgs {
            let mut args = vec![
                self.0.join("project.3mf").display().to_string(),
                "--bundle".into(),
                self.0.join("bundle.ini").display().to_string(),
                "--printer".into(),
                "Fixture (0.4 nozzle)".into(),
            ];
            args.extend(extra.iter().map(|s| s.to_string()));
            crate::parse_slice(&args).unwrap()
        }
        fn args(&self, extra: &[&str]) -> SliceArgs {
            let mut args = vec![
                self.0.join("project.3mf").display().to_string(),
                "--bundle".into(),
                self.0.join("bundle.ini").display().to_string(),
                "--printer".into(),
                "Fixture (0.4 nozzle)".into(),
                "--print".into(),
                "Default process".into(),
                "--filament".into(),
                "Default PLA".into(),
            ];
            args.extend(extra.iter().map(|s| s.to_string()));
            crate::parse_slice(&args).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    use threemf::Flavour;

    #[test]
    fn orca_project_selects_model_variant_inherits_and_user_copy_with_vendor() {
        for identity in [
            r#""printer_model":"Snapmaker U1","printer_variant":"0.4""#,
            r#""printer_model":"Snapmaker U1","nozzle_diameter":["0.4","0.4","0.4","0.4"]"#,
            r#""printer_settings_id":"My renamed U1","inherits":"Snapmaker U1 (0.4 nozzle)""#,
            r#""printer_model":"Snapmaker U1","inherits":"Snapmaker U1 (0.4 nozzle)""#,
            r#""printer_settings_id":"Snapmaker U1 (0.2 nozzle)","inherits":"Snapmaker U1 (0.4 nozzle)""#,
            r#""printer_settings_id":"Snapmaker U1 (0.4 nozzle) - Copy (modified)""#,
            // Physical identity wins over stale preset names and inherited presets.
            r#""printer_model":"Snapmaker U1","printer_variant":"0.4","inherits":"Snapmaker U1 (0.2 nozzle)","printer_settings_id":"Snapmaker U1 (0.2 nozzle)""#,
        ] {
            let f = Fixture::orca(&format!(
                r#"{{{identity},"different_settings_to_system":["wall_loops"],"wall_loops":"5"}}"#
            ));
            let a = f.orca_args(&["--plate", "1"]);
            let raw = threemf::load_config(&a.model).unwrap().unwrap();
            let mut selected = a.clone();
            select_printer(&mut selected, &ProjectSettings::parse(&raw).unwrap());
            assert_eq!(selected.vendor, "Snapmaker");
            assert_eq!(selected.printer, "Snapmaker U1 (0.4 nozzle)");
            let resolved = resolve(&a).unwrap();
            assert!(resolved.original.is_some());
            assert_eq!(resolved.config.vector_len("nozzle_diameter"), 4);
            assert_eq!(resolved.config.float_at("nozzle_diameter", 0), Some(0.4));
            assert_eq!(resolved.config.int("perimeters"), Some(5));
            // Regression: importing an object assigned to tool 3 must use the matched machine.
            mica_core::input::load_with_plate(
                std::slice::from_ref(&a.model),
                (0., 0.),
                false,
                true,
                0,
                &Default::default(),
                &resolved.config,
                &project::Mapper::default(),
                a.plate,
            )
            .unwrap();
        }
    }

    /// The project's whole configuration is used, not just its listed changes; `update` takes
    /// today's presets with the listed changes. Values the mapping rejects keep the preset's
    /// (with a note); absurd but readable values reach the safety checks, which refuse them
    /// with a message instead of slicing (or crashing).
    #[test]
    fn whole_embedded_configuration_is_used_and_checked() {
        let f = Fixture::orca(
            r#"{"printer_model":"Snapmaker U1","printer_variant":"0.4","different_settings_to_system":["wall_loops"],"wall_loops":"5","sparse_infill_density":"33%","top_shell_layers":"bad"}"#,
        );
        let updated = resolve(&f.orca_args(&["--project-settings", "update"]))
            .unwrap()
            .config;
        assert_eq!(updated.int("perimeters"), Some(5));
        assert_ne!(updated.float("fill_density"), Some(33.));
        let c = resolve(&f.orca_args(&[])).unwrap().config;
        assert_eq!(c.int("perimeters"), Some(5));
        assert_eq!(c.float("fill_density"), Some(33.));
        // `top_shell_layers` was rejected: the preset's value stays.
        assert_eq!(c.int("top_solid_layers"), updated.int("top_solid_layers"));
        let parse =
            |s: &[&str]| crate::parse_slice(&s.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert!(parse(&["x.3mf", "--project-settings", "update"]).is_ok());

        let f = Fixture::orca(
            r#"{"printer_model":"Snapmaker U1","printer_variant":"0.4","layer_height":"3","initial_layer_print_height":"3"}"#,
        );
        let c = resolve(&f.orca_args(&[])).unwrap().config;
        assert_eq!(c.float("layer_height"), Some(3.));
        let checked = mica_core::settings::PrintSettings::from_config(&c)
            .map_err(|e| e.to_string())
            .and_then(|s| mica_core::safety::settings(&s));
        assert!(
            checked.as_ref().is_err_and(|e| e.contains("layer height")),
            "{checked:?}"
        );
    }

    #[test]
    fn explicit_profiles_win_and_unknown_project_keeps_defaults() {
        let f = Fixture::orca(
            r#"{"printer_model":"Snapmaker U1","printer_variant":"0.2","printer_settings_id":"My U1"}"#,
        );
        let raw = threemf::load_config(f.0.join("project.3mf"))
            .unwrap()
            .unwrap();
        let settings = ProjectSettings::parse(&raw).unwrap();
        for flags in [
            vec!["--vendor", "Prusa"],
            vec!["--printer", crate::DEFAULT_PRINTER],
            vec!["--project-settings", "ignore"],
        ] {
            let mut a = f.orca_args(&flags);
            select_printer(&mut a, &settings);
            assert_eq!(a.vendor, "Prusa");
            assert_eq!(a.printer, crate::DEFAULT_PRINTER);
        }
        let mut a = f.orca_args(&[
            "--vendor",
            "Snapmaker",
            "--printer",
            "Snapmaker U1 (0.4 nozzle)",
        ]);
        select_printer(&mut a, &settings);
        assert_eq!(a.printer, "Snapmaker U1 (0.4 nozzle)");
        assert!(resolve(&a).unwrap().original.is_none());
        let mut a = f.orca_args(&[
            "--print",
            "Explicit process",
            "--filament",
            "Explicit filament",
        ]);
        select_printer(&mut a, &settings);
        assert_eq!(a.printer, "Snapmaker U1 (0.2 nozzle)");
        assert_eq!(a.print, "Explicit process");
        assert_eq!(a.filament, "Explicit filament");

        let f = Fixture::orca(
            r#"{"printer_model":"Unknown printer","printer_variant":"0.4","printer_settings_id":"Unknown - Copy","inherits":"Unknown base","different_settings_to_system":["wall_loops"],"wall_loops":"5"}"#,
        );
        let mut a = f.orca_args(&[]);
        a.bundle = Some(f.0.join("bundle.ini"));
        a.profiles_dir = None;
        let resolved = resolve(&a).unwrap();
        assert!(resolved.original.is_none());
        assert_eq!(resolved.config.vector_len("nozzle_diameter"), 1);
        assert_eq!(
            resolved.config.raw("printer_settings_id").as_deref(),
            Some(crate::DEFAULT_PRINTER)
        );
        assert_eq!(resolved.config.int("perimeters"), Some(2));
    }

    #[test]
    fn matching_project_applies_presets_scoped_changes_and_each_filament() {
        let f = Fixture::new();
        let c = resolve(&f.project_args(&[])).unwrap().config;
        assert_eq!(c.int("perimeters"), Some(4));
        assert_eq!(c.float("fill_density"), Some(12.));
        assert_eq!(c.float_at("temperature", 0), Some(215.));
        assert_eq!(c.float_at("temperature", 1), Some(245.));
        assert_eq!(c.float_at("filament_density", 0), Some(1.2));
        assert_eq!(c.float_at("filament_density", 1), Some(1.3));
        assert_eq!(
            c.raw("filament_settings_id").as_deref(),
            Some("\"Default PLA\";\"Project PLA\"")
        );
        assert_eq!(c.bool("support_material_auto"), Some(false));
        assert_eq!(c.string("support_material_style"), Some("organic"));
        // Prusa projects list no changes: `update` is today's presets.
        let c = resolve(&f.project_args(&["--project-settings", "update"]))
            .unwrap()
            .config;
        assert_eq!(c.int("perimeters"), Some(4));
        assert_eq!(c.float_at("temperature", 1), Some(230.));
        // Explicit presets other than the project's win for their scope: the process values
        // go, and the filament values of the tool whose project filament differs.
        let c = resolve(&f.args(&[])).unwrap().config;
        assert_eq!(c.int("perimeters"), Some(2));
        assert_ne!(c.float("fill_density"), Some(12.));
        assert_eq!(c.float_at("temperature", 0), Some(215.));
        assert_eq!(c.float_at("temperature", 1), Some(210.));
        assert_eq!(c.float_at("filament_density", 1), Some(1.2));
    }

    /// A project whose filament is not installed slices with a filament for its printer (one
    /// of the project's material type), never the CORE One default it is not compatible with
    /// (overnight stability campaign); without a project too, and the process likewise.
    #[test]
    fn missing_project_filament_falls_back_to_a_compatible_one() {
        let f = Fixture::directory();
        let dir = &f.0;
        std::fs::write(
            dir.join("bundle.ini"),
            format!(
                "[printer:Fixture (0.4 nozzle)]\nnozzle_diameter = 0.4\n\
                 [print:{print}]\nperimeters = 3\ncompatible_printers = \"Other\"\n\
                 [print:0.20mm SPEED @FIXTURE]\nperimeters = 5\n\
                 [filament:{filament}]\ntemperature = 215\ncompatible_printers = \"Other\"\n\
                 [filament:Generic PLA @FIXTURE]\ntemperature = 205\nfilament_type = PLA\n\
                 [filament:Generic PETG @FIXTURE]\ntemperature = 240\nfilament_type = PETG\n",
                print = crate::DEFAULT_PRINT,
                filament = crate::DEFAULT_FILAMENT,
            ),
        )
        .unwrap();
        let mut project = threemf::load_3mf(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/models/painted-cube.3mf"),
        )
        .unwrap();
        project.config = Some(threemf::RawConfig {
            path: "Metadata/Slic3r_PE.config".into(),
            text: "; printer_settings_id = Fixture (0.4 nozzle)\n; print_settings_id = Missing process\n; filament_settings_id = \"Missing PETG\"\n; filament_type = PETG\n; fill_density = 12%\n".into(),
        });
        threemf::write(dir.join("project.3mf"), &project, Flavour::Prusa).unwrap();
        let c = resolve(&f.project_args(&[])).unwrap().config;
        assert_eq!(
            c.raw("filament_settings_id").as_deref(),
            Some("\"Generic PETG @FIXTURE\"")
        );
        assert_eq!(c.float_at("temperature", 0), Some(240.));
        // The process: the default's counterpart for this printer.
        assert_eq!(c.int("perimeters"), Some(5));
        // The project's own values still apply.
        assert_eq!(c.float("fill_density"), Some(12.));
        let a = crate::parse_slice(&[
            "x.stl".to_string(),
            "--bundle".into(),
            dir.join("bundle.ini").display().to_string(),
            "--printer".into(),
            "Fixture (0.4 nozzle)".into(),
        ])
        .unwrap();
        let c = resolve(&a).unwrap().config;
        assert_eq!(
            c.raw("filament_settings_id").as_deref(),
            Some("\"Generic PLA @FIXTURE\"")
        );
        assert_eq!(
            fallback(
                "filament",
                "Generic PLA @FIXTURE",
                &["Generic PLA @FIXTURE".into()],
                None,
                "x"
            ),
            "Generic PLA @FIXTURE"
        );
    }

    #[test]
    fn ignore_and_mismatched_default_keep_selected_profiles_but_use_overrides_decision() {
        let f = Fixture::new();
        for options in [
            vec!["--project-settings", "ignore"],
            vec!["--printer", "Other (0.4 nozzle)"],
        ] {
            let c = resolve(&f.args(&options)).unwrap().config;
            assert_eq!(c.int("perimeters"), Some(2));
            assert_eq!(c.float_at("temperature", 1), Some(210.));
        }
        let c = resolve(&f.project_args(&[
            "--printer",
            "Other (0.4 nozzle)",
            "--project-settings",
            "use",
        ]))
        .unwrap()
        .config;
        assert_eq!(c.int("perimeters"), Some(4));
        let parse =
            |s: &[&str]| crate::parse_slice(&s.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert!(parse(&["x.3mf", "--project-settings", "invalid"]).is_err());
        assert!(parse(&["x.3mf", "--project-settings"]).is_err());
    }
}
