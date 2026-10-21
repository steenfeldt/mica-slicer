// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Format/3mf.cpp :: _3MF_Importer::_handle_start_config_metadata
// Source: prusaslicer@b028299c77 src/libslic3r/Model.cpp :: ModelVolume::type_from_string
// Source: orca@8500fcdcca src/libslic3r/Format/bbs_3mf.cpp :: _BBS_3MF_Importer::_handle_start_config_metadata
// Source: prusaslicer@b028299c77 src/libslic3r/Format/3mf.cpp :: _3MF_Importer::_handle_start_shape_configuration
// Source: orca@8500fcdcca src/libslic3r/Model.cpp :: ModelVolume::type_from_string
// Changes: Preserve raw settings and structural metadata; normalize both volume dialects and retain per-instance plates without applying settings; keep a part's `<slic3rpe:text>` / `<slic3rpe:shape>` emboss source as prefixed metadata.
// Upstream attribution (3mf.cpp):
// Copyright (c) Prusa Research 2018 - 2023 Oleksandra Iushchenko @YuSanka, David Kocík @kocikdav, Enrico Turri @enricoturri1966, Lukáš Matěna @lukasmatena, Lukáš Hejl @hejllukas, Filip Sykala @Jony01, Vojtěch Bubník @bubnikv, Tomáš Mészáros @tamasmeszaros
// Copyright (c) 2020 Henner Zeller
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher
//
// Upstream attribution (Model.cpp):
// Copyright (c) Prusa Research 2016 - 2023 Tomáš Mészáros @tamasmeszaros, Oleksandra Iushchenko @YuSanka, David Kocík @kocikdav, Enrico Turri @enricoturri1966, Lukáš Matěna @lukasmatena, Vojtěch Bubník @bubnikv, Lukáš Hejl @hejllukas, Filip Sykala @Jony01, Vojtěch Král @vojtechkral
// Copyright (c) 2021 Boleslaw Ciesielski
// Copyright (c) 2019 John Drake @foxox
// Copyright (c) 2019 Sijmen Schoon
// Copyright (c) Slic3r 2014 - 2016 Alessandro Ranellucci @alranel
// Copyright (c) 2015 Maksim Derbasov @ntfshard
//
// ported from lib/Slic3r/Model.pm:
// Copyright (c) Prusa Research 2016 - 2022 Vojtěch Bubník @bubnikv, Enrico Turri @enricoturri1966
// Copyright (c) Slic3r 2012 - 2016 Alessandro Ranellucci @alranel
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher

use super::xml::{get, integer, number, walk};
use super::{PartKind, Plate, PlateInstance, Project, Settings, ThreeMfError, invalid};
use std::collections::{BTreeMap, BTreeSet};
use std::io::BufRead;

#[derive(Default)]
pub(super) struct Metadata {
    pub objects: BTreeMap<u32, ObjectMeta>,
    pub plates: Vec<PlateMeta>,
}
#[derive(Default)]
pub(super) struct ObjectMeta {
    pub values: Settings,
    pub parts: Vec<PartMeta>,
}
#[derive(Default, Clone)]
pub(super) struct PartMeta {
    pub id: Option<u32>,
    pub range: Option<(usize, usize)>,
    pub values: Settings,
}
#[derive(Default)]
pub(super) struct PlateMeta {
    values: Settings,
    instances: Vec<Settings>,
}

pub(super) fn parse(
    reader: impl BufRead,
    ignored: &mut BTreeSet<String>,
) -> Result<Metadata, ThreeMfError> {
    let mut result = Metadata::default();
    let mut object: Option<(u32, ObjectMeta)> = None;
    let mut part: Option<PartMeta> = None;
    let mut plate: Option<PlateMeta> = None;
    let mut instance: Option<Settings> = None;
    walk(reader, "config", |name, attrs, stack| {
        if let Some(a) = attrs {
            match name {
                "object" if stack.len() == 1 => {
                    object = Some((integer(a, "id")?, ObjectMeta::default()));
                }
                "part" | "volume" if object.is_some() => {
                    if part.is_some() {
                        return Err(invalid("nested part metadata"));
                    }
                    let range = match (get(a, "firstid"), get(a, "lastid")) {
                        (Some(first), Some(last)) => Some((number(first)?, number(last)?)),
                        (None, None) => None,
                        _ => return Err(invalid("incomplete triangle range")),
                    };
                    let mut values = Settings::new();
                    for key in ["subtype", "volume_type"] {
                        if let Some(v) = get(a, key) {
                            values.insert(key.into(), v.into());
                        }
                    }
                    part = Some(PartMeta {
                        id: get(a, "id").map(number).transpose()?,
                        range,
                        values,
                    });
                }
                "plate" => plate = Some(PlateMeta::default()),
                "model_instance" if plate.is_some() => instance = Some(Settings::new()),
                "metadata" => {
                    if let (Some(k), Some(v)) = (get(a, "key"), get(a, "value")) {
                        let values = if let Some(i) = &mut instance {
                            Some(i)
                        } else if let Some(p) = &mut plate {
                            Some(&mut p.values)
                        } else if get(a, "type") != Some("object")
                            && let Some(p) = &mut part
                        {
                            Some(&mut p.values)
                        } else {
                            object.as_mut().map(|(_, o)| &mut o.values)
                        };
                        if let Some(values) = values {
                            values.insert(k.into(), v.into());
                        }
                    }
                }
                // PrusaSlicer's / Orca's editable emboss source (`<slic3rpe:text …/>`,
                // `<slic3rpe:shape …/>`): kept with the part as prefixed metadata.
                "shape" | "text" if part.is_some() => {
                    let prefix = if name == "text" {
                        crate::emboss::config::TEXT_PREFIX
                    } else {
                        crate::emboss::config::SHAPE_PREFIX
                    };
                    if let Some(p) = &mut part {
                        for (k, v) in a {
                            let k = k.rsplit(':').next().unwrap_or(k);
                            p.values.insert(format!("{prefix}{k}"), v.clone());
                        }
                    }
                }
                "shape" | "text" | "shape_configuration" | "text_configuration" => {
                    ignored.insert("editable text/shape source".into());
                }
                _ => {}
            }
        } else {
            match name {
                "part" | "volume" => {
                    if let Some(p) = part.take() {
                        object
                            .as_mut()
                            .ok_or_else(|| invalid("part without object"))?
                            .1
                            .parts
                            .push(p);
                    }
                }
                "object" => {
                    if let Some((id, o)) = object.take()
                        && result.objects.insert(id, o).is_some()
                    {
                        return Err(invalid("duplicate object metadata"));
                    }
                }
                "model_instance" => {
                    if let Some(i) = instance.take() {
                        plate
                            .as_mut()
                            .ok_or_else(|| invalid("instance without plate"))?
                            .instances
                            .push(i);
                    }
                }
                "plate" => {
                    if let Some(p) = plate.take() {
                        result.plates.push(p);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    })?;
    Ok(result)
}

pub(super) fn kind(values: &Settings) -> PartKind {
    let value = ["volume_type", "part_type", "subtype", "modifier"]
        .into_iter()
        .find_map(|k| values.get(k));
    match value.map(String::as_str).unwrap_or("ModelPart") {
        "ModelPart" | "normal_part" | "0" => PartKind::Model,
        "NegativeVolume" | "negative_part" => PartKind::NegativeVolume,
        "ParameterModifier" | "modifier_part" | "1" => PartKind::Modifier,
        "SupportEnforcer" | "support_enforcer" => PartKind::SupportEnforcer,
        "SupportBlocker" | "support_blocker" => PartKind::SupportBlocker,
        other => PartKind::Unknown(other.into()),
    }
}

pub(super) fn split(values: &Settings) -> (Settings, Settings) {
    values
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .partition(|(k, _)| {
            !matches!(
                k.as_str(),
                "name"
                    | "matrix"
                    | "volume_type"
                    | "part_type"
                    | "subtype"
                    | "modifier"
                    | "mesh_shared"
                    | "source_file"
                    | "source_object_id"
                    | "source_volume_id"
            ) && !k.starts_with("source_")
                && !crate::emboss::config::is_emboss_key(k)
        })
}

pub(super) fn assign_plates(
    project: &mut Project,
    root: &str,
    plates: Vec<PlateMeta>,
) -> Result<(), ThreeMfError> {
    let mut ids = BTreeSet::new();
    for p in plates {
        let id: u32 = number(
            p.values
                .get("plater_id")
                .ok_or_else(|| invalid("plate missing plater_id"))?,
        )?;
        if !ids.insert(id) {
            return Err(invalid("duplicate plate ID"));
        }
        let mut plate = Plate {
            id,
            name: p.values.get("plater_name").cloned().unwrap_or_default(),
            instances: Vec::new(),
            metadata: p.values,
        };
        for i in p.instances {
            let object_id = integer(&i, "object_id")?;
            let instance = integer(&i, "instance_id")? as usize;
            let object = project
                .objects
                .iter()
                .position(|o| o.source.path == root && o.source.id == object_id)
                .ok_or_else(|| invalid(format!("plate {id}: unknown object {object_id}")))?;
            let build = project.objects[object]
                .instances
                .get_mut(instance)
                .ok_or_else(|| invalid(format!("plate {id}: unknown instance {instance}")))?;
            if build.plate.replace(id).is_some() {
                return Err(invalid("instance assigned to more than one plate"));
            }
            plate.instances.push(PlateInstance { object, instance });
        }
        project.plates.push(plate);
    }
    Ok(())
}
