// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Format/3mf.cpp :: _3MF_Importer
// Source: orca@8500fcdcca src/libslic3r/Format/bbs_3mf.cpp :: _BBS_3MF_Importer
// Changes: Streaming quick-xml/zip reader, archive-scoped resource IDs, checked geometry/transforms, f64 source vertices beside the f32 slicing mesh and compact facet streams; height range IDs follow dialect-specific model order; literal hashes in member paths.
// Changes: Model parts queued together are decompressed and parsed in parallel, then consumed in queue order.
// Changes: An embossed SVG shape's file (`filepath3mf`, as `_extract_embossed_svg_shape_file`) is read into its part's metadata.
// Upstream attribution (3mf.cpp):
// Copyright (c) Prusa Research 2018 - 2023 Oleksandra Iushchenko @YuSanka, David Kocík @kocikdav, Enrico Turri @enricoturri1966, Lukáš Matěna @lukasmatena, Lukáš Hejl @hejllukas, Filip Sykala @Jony01, Vojtěch Bubník @bubnikv, Tomáš Mészáros @tamasmeszaros
// Copyright (c) 2020 Henner Zeller
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher
//

use super::xml::{get, integer, real, required, walk};
use super::{Instance, Project, RawConfig, ResourceId, Settings, ThreeMfError, Transform, invalid};
use crate::mesh::TriangleMesh;
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{BufReader, Read, Seek};
use zip::ZipArchive;

#[derive(Default)]
pub(super) struct Resource {
    pub name: String,
    pub mesh: TriangleMesh,
    pub vertices: Vec<[f64; 3]>,
    pub painting: crate::painting::FacetPainting,
    pub components: Vec<Component>,
    pub core_type: String,
}
#[derive(Clone)]
pub(super) struct Component {
    pub source: ResourceId,
    pub transform: Transform,
}
#[derive(Default)]
struct Model {
    objects: BTreeMap<u32, Resource>,
    object_order: Vec<u32>,
    build: Vec<(Component, bool)>,
}

pub(super) fn load(mut reader: impl Read + Seek) -> Result<Project, ThreeMfError> {
    // The compressed archive is held in memory once, so independent model parts can be
    // decompressed and parsed by several workers, each with its own cheap archive clone.
    // The archive's offsets are absolute in the stream, wherever it is positioned now.
    let mut bytes = Vec::new();
    reader.seek(std::io::SeekFrom::Start(0))?;
    reader.read_to_end(&mut bytes)?;
    let mut zip = ZipArchive::new(std::io::Cursor::new(std::sync::Arc::<[u8]>::from(bytes)))?;
    let mut entries = BTreeMap::new();
    let mut ignored = BTreeSet::new();
    for i in 0..zip.len() {
        let file = zip.by_index(i)?;
        if file.is_dir() {
            continue;
        }
        let name = archive_path("", file.name())?;
        if entries.insert(name.clone(), i).is_some() {
            return Err(invalid(format!("duplicate ZIP entry {name}")));
        }
        let lower = name.to_ascii_lowercase();
        let feature = if lower.starts_with(super::TEXTURES) {
            // Mica's custom texture images, read below.
            None
        } else if lower.ends_with(".png") || lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
            Some("thumbnails/images")
        } else if lower.contains("layer_height")
            && lower != "metadata/slic3r_pe_layer_heights_profile.txt"
            && lower != "metadata/layer_heights_profile.txt"
        {
            Some("variable layer height")
        } else if lower.contains("cut") {
            Some("cut information")
        } else if name == super::custom_gcode::PRUSA_FILE || name == super::custom_gcode::ORCA_FILE
        {
            None
        } else if lower.contains("custom_gcode") || lower.contains("custom_g_code") {
            Some("custom G-code")
        } else if lower.ends_with(".gcode") || lower.ends_with(".gcode.3mf") {
            Some("embedded sliced G-code")
        } else {
            None
        };
        if let Some(feature) = feature {
            ignored.insert(feature.into());
        }
    }
    let root_targets = relationships(&mut zip, &entries, "")?;
    let root = root_targets
        .first()
        .cloned()
        .unwrap_or_else(|| "3D/3dmodel.model".into());
    let mut queue = VecDeque::from([root.clone()]);
    let mut loaded = BTreeSet::new();
    let mut resources = BTreeMap::new();
    let mut resource_order = Vec::new();
    let mut build = Vec::new();
    // Breadth-first over model parts, exactly as one queue: every wave first parses the parts
    // queued so far in parallel, then consumes them in queue order (errors, ignored features and
    // resource order therefore come out as from a sequential walk).
    let mut parsed: BTreeMap<String, Result<(Model, BTreeSet<String>), ThreeMfError>> =
        BTreeMap::new();
    while let Some(path) = queue.pop_front() {
        if !loaded.insert(path.clone()) {
            continue;
        }
        if !parsed.contains_key(&path) {
            let mut wave: Vec<&String> = std::iter::once(&path)
                .chain(queue.iter())
                .filter(|p| !loaded.contains(*p) || **p == path)
                .collect();
            wave.sort();
            wave.dedup();
            let results: Vec<_> = wave
                .par_iter()
                .map(|p| {
                    let mut zip = zip.clone();
                    let result = (|| {
                        let index = *entries
                            .get(*p)
                            .ok_or_else(|| invalid(format!("missing model entry {p}")))?;
                        let file = zip.by_index(index)?;
                        let mut ignored = BTreeSet::new();
                        let model = parse_model(BufReader::new(file), p, &mut ignored)
                            .map_err(|e| invalid(format!("{p}: {e}")))?;
                        Ok((model, ignored))
                    })();
                    ((*p).clone(), result)
                })
                .collect();
            parsed.extend(results);
        }
        let (model, features) = parsed
            .remove(&path)
            .expect("every queued model part is parsed before it is consumed")?;
        ignored.extend(features);
        resource_order.extend(model.object_order.iter().map(|&id| ResourceId {
            path: path.clone(),
            id,
        }));
        for resource in model.objects.values() {
            queue.extend(resource.components.iter().map(|c| c.source.path.clone()));
        }
        queue.extend(model.build.iter().map(|(c, _)| c.source.path.clone()));
        if path == root {
            build = model.build;
        }
        resources.extend(model.objects.into_iter().map(|(id, r)| {
            (
                ResourceId {
                    path: path.clone(),
                    id,
                },
                r,
            )
        }));
        queue.extend(relationships(&mut zip, &entries, &path)?);
    }
    if build.is_empty() {
        return Err(invalid("no build items"));
    }
    let metadata_path = [
        "Metadata/model_settings.config",
        "Metadata/Slic3r_PE_model.config",
    ]
    .into_iter()
    .find(|p| entries.contains_key(*p));
    let metadata = if let Some(path) = metadata_path {
        super::metadata::parse(BufReader::new(zip.by_index(entries[path])?), &mut ignored)?
    } else {
        super::metadata::Metadata::default()
    };
    let mut project = Project {
        ignored_features: ignored,
        ..Project::default()
    };
    if let Some(&i) = entries.iter().find_map(|(name, i)| {
        name.eq_ignore_ascii_case(crate::full_spectrum::ARCHIVE_PATH)
            .then_some(i)
    }) {
        let mut text = String::new();
        zip.by_index(i)?.read_to_string(&mut text)?;
        project.full_spectrum_json = Some(text);
    }
    let mut object_indices = BTreeMap::new();
    for (mut component, printable) in build {
        // Prusa emits extra instances as wrapper resources without metadata. Collapse
        // those aliases, while keeping Orca's separately configured objects distinct.
        if metadata_path == Some("Metadata/Slic3r_PE_model.config") {
            let mut visited = BTreeSet::new();
            loop {
                if !visited.insert(component.source.clone()) {
                    return Err(invalid("component cycle in Prusa instance alias"));
                }
                if component.source.path == root
                    && metadata.objects.contains_key(&component.source.id)
                {
                    break;
                }
                let resource = resources.get(&component.source).ok_or_else(|| {
                    invalid(format!("missing instance object {}", component.source.id))
                })?;
                if resource.components.len() != 1 {
                    break;
                }
                let child = &resource.components[0];
                component.transform = component.transform.compose(child.transform);
                component.source = child.source.clone();
            }
        }
        let index = if let Some(&index) = object_indices.get(&component.source) {
            index
        } else {
            let index = project.objects.len();
            let object =
                super::assemble::object(&component.source, &root, &resources, &metadata.objects)?;
            object_indices.insert(component.source.clone(), index);
            project.objects.push(object);
            index
        };
        project.objects[index].instances.push(Instance {
            transform: component.transform,
            printable,
            plate: None,
        });
    }
    super::metadata::assign_plates(&mut project, &root, metadata.plates)?;
    // Embossed SVG shapes keep their source file in the archive (`filepath3mf`).
    for part in project.objects.iter_mut().flat_map(|o| o.parts.iter_mut()) {
        let key = format!("{}filepath3mf", crate::emboss::config::SHAPE_PREFIX);
        let Some(path) = part.metadata.get(&key) else {
            continue;
        };
        let Ok(name) = archive_path("", path.trim_start_matches('/')) else {
            continue;
        };
        if let Some(&i) = entries.get(&name) {
            let mut text = String::new();
            if zip
                .by_index(i)?
                .take(64 << 20)
                .read_to_string(&mut text)
                .is_ok()
            {
                part.metadata
                    .insert(crate::emboss::config::SVG_DATA_KEY.into(), text);
            }
        }
    }
    for path in [
        "Metadata/project_settings.config",
        "Metadata/Slic3r_PE.config",
    ] {
        if let Some(&i) = entries.get(path) {
            let mut text = String::new();
            zip.by_index(i)?.read_to_string(&mut text)?;
            project.config = Some(RawConfig {
                path: path.into(),
                text,
            });
            break;
        }
    }
    if let Some(&i) = entries.get(super::custom_gcode::ORCA_FILE) {
        project.custom_gcodes =
            super::custom_gcode::parse_orca(std::io::BufReader::new(zip.by_index(i)?))?;
    } else if let Some(&i) = entries.get(super::custom_gcode::PRUSA_FILE) {
        project.custom_gcodes =
            super::custom_gcode::parse_prusa(std::io::BufReader::new(zip.by_index(i)?))?;
    }
    project
        .custom_gcodes
        .retain(|_, info| !info.items.is_empty());
    for info in project.custom_gcodes.values_mut() {
        info.check_mode();
    }
    if let Some(&i) = entries.get("Metadata/Prusa_Slicer_wipe_tower_information.xml") {
        project.wipe_towers = super::tower::parse(std::io::BufReader::new(zip.by_index(i)?))?;
    }
    if !project.wipe_towers.contains_key(&0)
        && let Some(config) = &project.config
        && let Some(placement) = super::tower::legacy(config)?
    {
        project.wipe_towers.insert(0, placement);
    }
    if let Some(config) = &project.config {
        for (bed, placement) in super::write_config::orca_towers(config)? {
            project.wipe_towers.entry(bed).or_insert(placement);
        }
    }
    // Prusa creates model objects in mesh-resource order; Orca creates them at the first
    // build occurrence. Neither dialect's range IDs are 3MF resource IDs.
    let prusa_range_indices: Vec<_> = resource_order
        .iter()
        .filter_map(|source| object_indices.get(source).copied())
        .collect();
    for path in [
        "Metadata/Prusa_Slicer_layer_config_ranges.xml",
        "Metadata/layer_config_ranges.xml",
    ] {
        if let Some(&i) = entries.get(path) {
            for (id, ranges) in super::ranges::parse(BufReader::new(zip.by_index(i)?))? {
                let index = if path.contains("Prusa_Slicer") {
                    *prusa_range_indices
                        .get(id - 1)
                        .ok_or_else(|| invalid("height range object index out of bounds"))?
                } else {
                    id - 1
                };
                let object = project
                    .objects
                    .get_mut(index)
                    .ok_or_else(|| invalid("height range object index out of bounds"))?;
                object.height_ranges = ranges;
            }
        }
    }
    // Layer height profiles are indexed like the height ranges; brim ears are Orca-only.
    for path in [
        "Metadata/Slic3r_PE_layer_heights_profile.txt",
        "Metadata/layer_heights_profile.txt",
    ] {
        if let Some(&i) = entries.get(path) {
            let mut text = String::new();
            zip.by_index(i)?.read_to_string(&mut text)?;
            for (id, profile) in super::layer_heights::parse_profiles(&text) {
                let index = if path.contains("Slic3r_PE") {
                    prusa_range_indices.get(id - 1).copied()
                } else {
                    Some(id - 1)
                };
                if let Some(object) = index.and_then(|i| project.objects.get_mut(i)) {
                    object.layer_height_profile = profile;
                }
            }
        }
    }
    if let Some(&i) = entries.get("Metadata/brim_ear_points.txt") {
        let mut text = String::new();
        zip.by_index(i)?.read_to_string(&mut text)?;
        for (id, points) in super::layer_heights::parse_brim_points(&text) {
            if let Some(object) = project.objects.get_mut(id - 1) {
                object.brim_points = points;
            }
        }
    }
    super::marker::restore(&mut zip, &mut project)?;
    super::register_textures(&mut zip)?;
    Ok(project)
}

fn relationships<R: Read + Seek>(
    zip: &mut ZipArchive<R>,
    entries: &BTreeMap<String, usize>,
    model: &str,
) -> Result<Vec<String>, ThreeMfError> {
    let path = if model.is_empty() {
        "_rels/.rels".into()
    } else {
        let (dir, name) = model.rsplit_once('/').unwrap_or(("", model));
        format!("{dir}/_rels/{name}.rels")
            .trim_start_matches('/')
            .to_owned()
    };
    let Some(&i) = entries.get(&path) else {
        return Ok(Vec::new());
    };
    let mut targets = Vec::new();
    walk(
        BufReader::new(zip.by_index(i)?),
        "Relationships",
        |name, attrs, _| {
            if name == "Relationship"
                && let Some(a) = attrs
                && get(a, "Type").is_some_and(|t| t.ends_with("/3dmodel"))
            {
                if get(a, "TargetMode") == Some("External") {
                    return Err(invalid("external model relationship"));
                }
                targets.push(archive_path(model, required(a, "Target")?)?);
            }
            Ok(())
        },
    )?;
    Ok(targets)
}

fn parse_model(
    reader: impl std::io::BufRead,
    path: &str,
    ignored: &mut BTreeSet<String>,
) -> Result<Model, ThreeMfError> {
    let mut model = Model::default();
    let mut unit = 1.0;
    let mut current: Option<(u32, Resource)> = None;
    walk(reader, "model", |name, attrs, stack| {
        let parent = stack.last().map(String::as_str).unwrap_or("");
        let Some(a) = attrs else {
            if name == "object" && parent == "resources" {
                let (id, mut resource) = current
                    .take()
                    .ok_or_else(|| invalid("object end without start"))?;
                if !resource.mesh.faces.is_empty() && !resource.components.is_empty() {
                    return Err(invalid("object contains both mesh and components"));
                }
                for face in &resource.mesh.faces {
                    if face
                        .iter()
                        .any(|&v| v as usize >= resource.mesh.vertices.len())
                    {
                        return Err(invalid(format!(
                            "object {id}: triangle vertex index out of range"
                        )));
                    }
                }
                // Match upstream orientation repair without changing connectivity or triangle count.
                if resource.mesh.volume() < 0.0 {
                    resource.painting.reverse_winding();
                    for f in &mut resource.mesh.faces {
                        f.swap(1, 2);
                    }
                }
                model.object_order.push(id);
                if model.objects.insert(id, resource).is_some() {
                    return Err(invalid(format!("duplicate object {id}")));
                }
            }
            return Ok(());
        };
        match (name, parent) {
            ("model", "") => {
                unit = match get(a, "unit").unwrap_or("millimeter") {
                    "micron" => 0.001,
                    "millimeter" => 1.0,
                    "centimeter" => 10.0,
                    "inch" => 25.4,
                    "foot" => 304.8,
                    "meter" => 1000.0,
                    other => return Err(invalid(format!("unsupported unit {other}"))),
                };
            }
            ("object", "resources") => {
                if current.is_some() {
                    return Err(invalid("nested object"));
                }
                let id = integer(a, "id")?;
                if id == 0 {
                    return Err(invalid("object ID must be positive"));
                }
                current = Some((
                    id,
                    Resource {
                        name: get(a, "name").unwrap_or("").into(),
                        core_type: get(a, "type").unwrap_or("model").into(),
                        ..Resource::default()
                    },
                ));
            }
            ("vertex", "vertices") => {
                let r = &mut current
                    .as_mut()
                    .ok_or_else(|| invalid("vertex outside object"))?
                    .1;
                let mut v = [0.0; 3];
                for (i, k) in ["x", "y", "z"].iter().enumerate() {
                    v[i] = real(required(a, k)?)? * unit;
                }
                if v.iter().any(|v| !v.is_finite() || !(*v as f32).is_finite()) {
                    return Err(invalid("vertex coordinate overflow"));
                }
                if r.mesh.vertices.len() >= u32::MAX as usize {
                    return Err(invalid("too many vertices"));
                }
                r.vertices.push(v);
                r.mesh.vertices.push(v.map(|v| v as f32));
            }
            ("triangle", "triangles") => {
                let r = &mut current
                    .as_mut()
                    .ok_or_else(|| invalid("triangle outside object"))?
                    .1;
                r.mesh
                    .faces
                    .push([integer(a, "v1")?, integer(a, "v2")?, integer(a, "v3")?]);
                for (key, value) in a {
                    let states = match key.rsplit(':').next().unwrap_or(key) {
                        "custom_supports" | "paint_supports" => Some(&mut r.painting.supports),
                        "custom_seam" | "paint_seam" => Some(&mut r.painting.seam),
                        "mmu_segmentation" | "paint_color" => Some(&mut r.painting.material),
                        "fuzzy_skin" | "paint_fuzzy_skin" => Some(&mut r.painting.fuzzy),
                        _ => None,
                    };
                    if let Some(states) = states {
                        states
                            .push((r.mesh.faces.len() - 1) as u32, value)
                            .map_err(|e| {
                                invalid(format!("triangle {} {key}: {e}", r.mesh.faces.len() - 1))
                            })?;
                    } else if key.contains("paint") && !value.is_empty() {
                        ignored.insert(format!("unknown painting attribute {key}"));
                    }
                    if matches!(key.as_str(), "pid" | "p1" | "p2" | "p3") {
                        ignored.insert("core material/color properties".into());
                    }
                }
            }
            ("component", "components") => {
                current
                    .as_mut()
                    .ok_or_else(|| invalid("component outside object"))?
                    .1
                    .components
                    .push(component(a, path, unit)?);
            }
            ("item", "build") => {
                let printable = match get(a, "printable").unwrap_or("1") {
                    "1" | "true" => true,
                    "0" | "false" => false,
                    _ => return Err(invalid("invalid printable flag")),
                };
                model.build.push((component(a, path, unit)?, printable));
            }
            ("object" | "vertex" | "triangle" | "component" | "item", _) => {
                return Err(invalid(format!("unexpected {name} in {parent}")));
            }
            _ => {}
        }
        Ok(())
    })?;
    Ok(model)
}

fn component(a: &Settings, path: &str, unit: f64) -> Result<Component, ThreeMfError> {
    let path = match get(a, "path") {
        Some(p) if !p.is_empty() => archive_path(path, p)?,
        _ => path.into(),
    };
    Ok(Component {
        source: ResourceId {
            path,
            id: integer(a, "objectid")?,
        },
        transform: transform(get(a, "transform"), unit)?,
    })
}

fn transform(value: Option<&str>, unit: f64) -> Result<Transform, ThreeMfError> {
    let Some(value) = value else {
        return Ok(Transform::IDENTITY);
    };
    let values = value
        .split_whitespace()
        .map(real)
        .collect::<Result<Vec<_>, _>>()?;
    if values.len() != 12 {
        return Err(invalid("transform must have 12 numbers"));
    }
    let mut transform = Transform::IDENTITY;
    for i in 0..3 {
        for j in 0..4 {
            transform.0[i][j] = values[j * 3 + i] * if j == 3 { unit } else { 1.0 };
        }
    }
    if transform.0.iter().flatten().any(|n| !n.is_finite())
        || !transform.determinant().is_finite()
        || transform.determinant() == 0.0
    {
        return Err(invalid("singular or overflowing transform"));
    }
    Ok(transform)
}

/// Resolve package URIs only; no path from an archive is ever opened on the filesystem.
fn archive_path(base: &str, target: &str) -> Result<String, ThreeMfError> {
    let mut decoded = Vec::with_capacity(target.len());
    let mut bytes = target.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let a = bytes.next().and_then(|b| (b as char).to_digit(16));
            let b = bytes.next().and_then(|b| (b as char).to_digit(16));
            decoded.push(match (a, b) {
                (Some(a), Some(b)) => (a * 16 + b) as u8,
                _ => return Err(invalid("invalid package URI escape")),
            });
        } else {
            decoded.push(b);
        }
    }
    let target = String::from_utf8(decoded).map_err(|_| invalid("non-UTF8 package URI"))?;
    if target.contains(['\\', ':', '\0', '?']) {
        return Err(invalid("invalid package URI"));
    }
    let mut parts: Vec<&str> = if target.starts_with('/') {
        Vec::new()
    } else {
        base.rsplit_once('/')
            .map_or(Vec::new(), |(dir, _)| dir.split('/').collect())
    };
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(invalid("package URI escapes archive"));
                }
            }
            p => parts.push(p),
        }
    }
    Ok(parts.join("/"))
}
