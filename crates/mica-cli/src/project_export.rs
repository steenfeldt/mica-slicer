// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Save the editable source project before the slicing adapter flattens instances or
//! filters plates. No slice geometry or preview caches enter the project archive.
use super::SliceArgs;
use mica_core::{config::Config, mesh::TriangleMesh, threemf::*};

pub(super) fn save(
    a: &SliceArgs,
    config: &Config,
    center: (f32, f32),
    original: Option<&RawConfig>,
    selected_orca: Option<&mica_profiles::orca::RawConfig>,
) -> Result<(), String> {
    let project = build(a, center, original)?;
    let path = a.export_3mf.as_ref().unwrap();
    let summary = finish(a, config, project, original, selected_orca, path)?;
    eprintln!("{summary}");
    Ok(())
}

/// The editable project of the input models (STLs placed on the bed at `center`, 3MF
/// projects merged as they are), with the object tool flags applied. `original` (the
/// resolution used embedded settings) makes projects with different settings an error.
pub(super) fn build(
    a: &SliceArgs,
    center: (f32, f32),
    original: Option<&RawConfig>,
) -> Result<Project, String> {
    let mut project = Project::default();
    let mut paths = vec![a.model.clone()];
    paths.extend(a.extra_models.clone());
    let only_stl = paths
        .iter()
        .all(|p| !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("3mf")));
    let mut next_plate = 1;
    for path in &paths {
        let mut part = if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("3mf"))
        {
            load_3mf(path).map_err(|e| e.to_string())?
        } else {
            let mut mesh = TriangleMesh::load_stl(path).map_err(|e| e.to_string())?;
            mesh.place_on_bed(center.0, center.1);
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let source = ResourceId {
                path: "3D/3dmodel.model".into(),
                id: 1,
            };
            Project {
                objects: vec![Object {
                    name: name.clone(),
                    source: source.clone(),
                    height_ranges: vec![],
                    layer_height_profile: vec![],
                    brim_points: vec![],
                    settings: Settings::new(),
                    metadata: Settings::new(),
                    instances: vec![Instance {
                        transform: Transform::IDENTITY,
                        printable: true,
                        plate: None,
                    }],
                    parts: vec![Part {
                        name,
                        kind: PartKind::Model,
                        source,
                        mesh,
                        source_mesh: None,
                        painting: Default::default(),
                        transform: Transform::IDENTITY,
                        settings: Settings::new(),
                        metadata: Settings::new(),
                    }],
                }],
                ..Project::default()
            }
        };
        if original.is_some()
            && project.config.is_some()
            && part.config.is_some()
            && project.config.as_ref().map(|c| (&c.path, &c.text))
                != part.config.as_ref().map(|c| (&c.path, &c.text))
        {
            return Err("cannot combine projects with different embedded settings; export each project separately".into());
        }
        if project.config.is_none() {
            project.config = part.config.take();
        }
        if let Some(mixes) = part.full_spectrum_json.take() {
            if project
                .full_spectrum_json
                .as_ref()
                .is_some_and(|existing| *existing != mixes)
            {
                return Err("cannot combine projects with different mixed color definitions; export each project separately".into());
            }
            project.full_spectrum_json = Some(mixes);
        }
        let object_base = project.objects.len();
        let plate_base = project.plates.len() as u32;
        let mut ids = std::collections::BTreeMap::new();
        for plate in &mut part.plates {
            // Preserve original IDs for a single source; remap only when merging archives.
            let id = if object_base == 0 {
                plate.id
            } else {
                next_plate
            };
            next_plate = next_plate.max(
                id.checked_add(1)
                    .ok_or("plate ID exceeds supported range")?,
            );
            ids.insert(plate.id, id);
            plate.id = id;
            for i in &mut plate.instances {
                i.object += object_base;
            }
        }
        for o in &mut part.objects {
            for i in &mut o.instances {
                if let Some(id) = i.plate {
                    i.plate = ids.get(&id).copied();
                }
            }
        }
        project.wipe_towers.extend(
            part.wipe_towers
                .into_iter()
                .map(|(i, v)| (i + plate_base, v)),
        );
        project.custom_gcodes.extend(
            part.custom_gcodes
                .into_iter()
                .map(|(i, v)| (i + plate_base, v)),
        );
        project.ignored_features.extend(part.ignored_features);
        project.objects.extend(part.objects);
        project.plates.extend(part.plates);
    }
    for (&id, &tool) in &a.object_extruders {
        let count = project.objects.len();
        let o = project
            .objects
            .get_mut(id - 1)
            .ok_or_else(|| format!("object {id} is outside 1..={count}"))?;
        o.settings.insert("extruder".into(), tool.to_string());
        for p in &mut o.parts {
            p.settings.insert("extruder".into(), tool.to_string());
        }
    }
    if a.extruder != 1 {
        for o in &mut project.objects {
            o.settings
                .entry("extruder".into())
                .or_insert_with(|| a.extruder.to_string());
        }
    }
    if only_stl && paths.len() > 1 {
        let mut x = 0.;
        for o in &mut project.objects {
            let (lo, hi) = o.parts[0].mesh.bounds();
            let width = hi[0] - lo[0];
            o.parts[0].mesh.translate([x - lo[0], 0., 0.]);
            x += width + 10.;
        }
    }
    if a.center.is_some() || (only_stl && paths.len() > 1) {
        let mut mesh = project.placed_model_mesh().map_err(|e| e.to_string())?;
        let (before, _) = mesh.bounds();
        mesh.place_on_bed(center.0, center.1);
        let (after, _) = mesh.bounds();
        for o in &mut project.objects {
            for i in &mut o.instances {
                for axis in 0..3 {
                    i.transform.0[axis][3] += f64::from(after[axis] - before[axis]);
                }
            }
        }
    }
    Ok(project)
}

/// Adds the settings (the resolved `config`, the project's own `original` values) and the
/// per-plate data the flags ask for to `project` and writes it to `path`; returns the summary
/// line.
pub(super) fn finish(
    a: &SliceArgs,
    config: &Config,
    mut project: Project,
    original: Option<&RawConfig>,
    selected_orca: Option<&mica_profiles::orca::RawConfig>,
    path: &std::path::Path,
) -> Result<String, String> {
    layer_profiles_and_ears(a, config, &mut project)?;
    let edits = explicit_edits(a, config);
    mica_profiles::project::SaveSettings {
        config,
        colours: &super::project_settings::colours(config),
        original,
        edits: &edits,
    }
    .write(&mut project, a.flavour.unwrap_or_default(), || {
        Ok(selected_orca.cloned().map(|raw| (raw, Default::default())))
    })?;
    for (axis, key) in ["wipe_tower_x", "wipe_tower_y", "wipe_tower_rotation_angle"]
        .into_iter()
        .enumerate()
    {
        if a.overrides.iter().any(|(k, _)| k == key) {
            let index = match a.plate {
                mica_core::input::PlateSelection::Plate(n) => (n - 1) as u32,
                _ => 0,
            };
            let value = config
                .raw(key)
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| v.is_finite())
                .ok_or_else(|| format!("invalid {key}"))?;
            project.wipe_towers.entry(index).or_insert([180., 140., 0.])[axis] = value;
        }
    }
    if !a.custom_gcode.is_empty() {
        let index = match a.plate {
            mica_core::input::PlateSelection::Plate(n) => (n - 1) as u32,
            _ => 0,
        };
        let tools = config.vector_len("nozzle_diameter").max(1);
        let object_tools = project
            .objects
            .iter()
            .map(|o| o.settings.get("extruder").cloned().unwrap_or_default())
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let info = project.custom_gcodes.remove(&index).unwrap_or_default();
        project.custom_gcodes.insert(
            index,
            super::custom_gcode::merge(info, &a.custom_gcode, config, tools, object_tools),
        );
    }
    project.custom_gcode_templates = CustomGCodeTemplates {
        color_change: config.string("color_change_gcode").unwrap_or("M600").into(),
        pause: config.string("pause_print_gcode").unwrap_or("M601").into(),
        template: config.string("template_custom_gcode").unwrap_or("").into(),
    };
    write(path, &project, a.flavour.unwrap_or_default())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(format!(
        "{} objects, {} plates → {} ({})",
        project.objects.len(),
        project.plates.len().max(1),
        path.display(),
        a.flavour.unwrap_or_default().name()
    ))
}

/// `--adaptive-layers` / `--smooth-layers`: an adaptive layer height profile per object (the
/// GUI's Adaptive and Smooth buttons). `--brim-ear X,Y,R`: a hand-placed ear on the bed, kept by
/// the object under it (the first object otherwise), in that object's coordinates.
fn layer_profiles_and_ears(
    a: &SliceArgs,
    config: &Config,
    project: &mut Project,
) -> Result<(), String> {
    use mica_core::layer_profile as lp;
    let placed = |o: &Object| -> Option<(TriangleMesh, Transform)> {
        let instance = o.instances.first()?;
        let mut mesh = TriangleMesh::default();
        for part in o.parts.iter().filter(|p| p.kind == PartKind::Model) {
            mica_core::input::append(&mut mesh, &part.mesh, instance.transform);
        }
        (!mesh.faces.is_empty()).then_some((mesh, instance.transform))
    };
    if let Some(quality) = a.adaptive_layers {
        for o in &mut project.objects {
            let Some((mesh, _)) = placed(o) else {
                continue;
            };
            let (lo, hi) = mesh.bounds();
            let tool = o
                .settings
                .get("extruder")
                .and_then(|v| v.parse::<usize>().ok())
                .filter(|t| *t > 0)
                .unwrap_or(a.extruder)
                - 1;
            let p = lp::SlicingParams::from_config(config, &[tool], f64::from(hi[2] - lo[2]));
            let mut profile = lp::adaptive(&p, &mesh, lo[2], quality);
            if let Some(radius) = a.smooth_layers {
                profile = lp::smooth(&profile, &p, radius, false);
            }
            o.layer_height_profile = profile;
        }
    }
    for &[x, y, r] in &a.brim_ears {
        let target = project
            .objects
            .iter()
            .position(|o| {
                placed(o).is_some_and(|(m, _)| {
                    let (lo, hi) = m.bounds();
                    (f64::from(lo[0])..=f64::from(hi[0])).contains(&x)
                        && (f64::from(lo[1])..=f64::from(hi[1])).contains(&y)
                })
            })
            .unwrap_or(0);
        let o = project
            .objects
            .get_mut(target)
            .ok_or("--brim-ear needs an object")?;
        let Some((_, transform)) = placed(o) else {
            return Err("--brim-ear needs a placed object".into());
        };
        let inverse = transform.inverse().ok_or("singular object transform")?;
        // Upstream places ears just below the bed (world Z -0.0001).
        let local = inverse.apply([x as f32, y as f32, -0.0001]);
        let origin = o.brim_frame_origin();
        let p: [f32; 3] = std::array::from_fn(|a| (f64::from(local[a]) - origin[a]) as f32);
        o.brim_points.push([p[0], p[1], p[2], r as f32]);
    }
    Ok(())
}

/// CLI vector overrides apply to every supplied slot; list the key for each filament.
fn explicit_edits(a: &SliceArgs, config: &Config) -> mica_profiles::project::Changes {
    use mica_core::config::{Scope, option_defs};
    let tools = config.vector_len("nozzle_diameter").max(1);
    let mut edits = mica_profiles::project::Changes {
        filaments: vec![Settings::new(); tools],
        ..Default::default()
    };
    for key in a.overrides.iter().map(|(k, _)| k.as_str()).chain(
        a.perimeter_generator
            .as_ref()
            .map(|_| "perimeter_generator"),
    ) {
        let key = match key {
            "skirt_loops" => "skirts",
            "brim_object_gap" => "brim_separation",
            k => k,
        };
        // Identity Orca aliases are canonicalised by the override adapter as well.
        let key = mica_profiles::orca::mapping::mapping_table()
            .get(key)
            .filter(|(_, conversion)| *conversion == "identity")
            .map_or(key, |(canonical, _)| *canonical);
        let Some(value) = config.raw(key) else {
            continue;
        };
        // Orca options of either schema (Snapmaker Orca's flow-variant set adds its own).
        match option_defs().get(key).map(|d| d.scope).or_else(|| {
            use mica_profiles::orca::Dialect;
            [Dialect::Orca242, Dialect::Snapmaker240]
                .into_iter()
                .find_map(|d| mica_profiles::orca::option_scope(key, d))
        }) {
            Some(Scope::Filament) => {
                for slot in &mut edits.filaments {
                    slot.insert(key.into(), value.clone());
                }
            }
            Some(Scope::Printer) => {
                edits.printer.insert(key.into(), value);
            }
            _ => {
                edits.process.insert(key.into(), value);
            }
        }
    }
    edits
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn save_keeps_full_project_and_applies_explicit_overrides() {
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/models/plates-orca.3mf");
        let path = std::env::temp_dir().join(format!("mica-cli-export-{}.3mf", std::process::id()));
        let args = vec![
            source.to_string_lossy().into_owned(),
            "--export-3mf".into(),
            path.to_string_lossy().into_owned(),
            "--object-extruder".into(),
            "2=4".into(),
            "--set".into(),
            "perimeters=5".into(),
            "--set".into(),
            "wipe_tower_x=22".into(),
            "--plate".into(),
            "2".into(),
        ];
        let args = super::super::parse_slice(&args).unwrap();
        let mut c = Config::defaults();
        c.set_raw("perimeters", "5").unwrap();
        c.set_raw("wipe_tower_x", "22").unwrap();
        save(
            &args,
            &c,
            (125., 110.),
            load_config(&source).unwrap().as_ref(),
            None,
        )
        .unwrap();
        let p = load_3mf(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(p.objects.len(), 4);
        assert_eq!(p.plates.len(), 2);
        assert_eq!(p.objects[1].settings["extruder"], "4");
        assert_eq!(p.wipe_towers[&0][0], 165.);
        assert_eq!(p.wipe_towers[&1][0], 22.);
        assert!(p.config.unwrap().text.contains("\"wall_loops\":\"5\""));
    }
    #[test]
    fn explicit_aliases_are_scoped_and_filament_keys_cover_all_slots() {
        let a = super::super::parse_slice(
            &[
                "fixture.3mf",
                "--set",
                "wall_loops=5",
                "--set",
                "nozzle_temperature=221,222,223,224",
                "--set",
                "retract_length=0.9",
                "--set",
                "enable_pressure_advance=1",
            ]
            .map(str::to_string),
        )
        .unwrap();
        let mut c = Config::defaults();
        c.set_raw("nozzle_diameter", "0.4,0.4,0.4,0.4").unwrap();
        // U1 imports this native filament option outside the canonical schema.
        c.set_raw("enable_pressure_advance", "0").unwrap();
        for (key, value) in &a.overrides {
            crate::overrides::apply(&mut c, key, value).unwrap();
        }
        let changes = explicit_edits(&a, &c);
        assert_eq!(
            changes.process,
            Settings::from([("perimeters".into(), "5".into())])
        );
        assert_eq!(
            changes.printer,
            Settings::from([("retract_length".into(), "0.9".into())])
        );
        assert_eq!(changes.filaments.len(), 4);
        for slot in changes.filaments {
            assert_eq!(slot["temperature"], "221,222,223,224");
            assert_eq!(slot["enable_pressure_advance"], "1");
            assert_eq!(slot.len(), 2);
        }
    }

    #[test]
    fn export_flag_validation() {
        let parse = |args: &[&str]| {
            super::super::parse_slice(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        };
        assert!(parse(&["x.3mf", "--flavour", "orca"]).is_err());
        assert!(parse(&["x.3mf", "--export-3mf", "out.3mf", "--flavour", "wrong"]).is_err());
        assert!(parse(&["x.3mf", "--export-3mf", "out.3mf", "-o", "out.3mf"]).is_err());
        assert_eq!(
            parse(&["x.3mf", "--export-3mf", "out.3mf", "--flavour", "prusa"])
                .unwrap()
                .flavour,
            Some(Flavour::Prusa)
        );
    }
}
