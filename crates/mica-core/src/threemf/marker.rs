// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Versioned, geometry-free sidecar. ZIP CRC/size guards prevent stale metadata from
//! overriding native edits made by another slicer which happens to retain our sidecar.
use super::{json::Json, *};
use crate::painting::PaintKind;
use std::io::{Read, Seek};

fn settings(s: &Settings) -> Json {
    Json::object(s.iter().map(|(k, v)| (k.clone(), Json::from(v.clone()))))
}
fn read_settings(j: &Json) -> Result<Settings, ThreeMfError> {
    j.map()?
        .iter()
        .map(|(k, v)| Ok((k.clone(), v.text()?.into())))
        .collect()
}
fn source(s: &ResourceId) -> Json {
    Json::object([
        ("path", Json::from(s.path.clone())),
        ("id", Json::number(s.id)),
    ])
}
fn read_source(j: &Json) -> Result<ResourceId, ThreeMfError> {
    Ok(ResourceId {
        path: j.get("path")?.text()?.into(),
        id: j.get("id")?.num()?,
    })
}
fn matrix(t: Transform) -> Json {
    Json::Array(t.0.into_iter().flatten().map(Json::number).collect())
}
fn read_matrix(j: &Json) -> Result<Transform, ThreeMfError> {
    let a = j.array()?;
    if a.len() != 12 {
        return Err(invalid("marker transform length"));
    }
    let mut t = Transform::IDENTITY;
    for (v, j) in t.0.iter_mut().flatten().zip(a) {
        *v = j.num()?;
    }
    if t.0.iter().flatten().any(|v| !v.is_finite())
        || !t.determinant().is_finite()
        || t.determinant() == 0.
    {
        return Err(invalid("invalid marker transform"));
    }
    Ok(t)
}
pub(super) fn manifest<R: Read + Seek>(zip: &mut zip::ZipArchive<R>) -> Result<Json, ThreeMfError> {
    let mut files = Vec::new();
    for i in 0..zip.len() {
        let f = zip.by_index(i)?;
        if f.name() != "Metadata/mica.json" {
            files.push((
                f.name().to_owned(),
                Json::Array(vec![Json::number(f.crc32()), Json::number(f.size())]),
            ));
        }
    }
    Ok(Json::object(files))
}
pub(super) fn encode(p: &Project, flavour: Flavour, manifest: Json) -> Json {
    let objects = p
        .objects
        .iter()
        .map(|o| {
            Json::object([
                ("name", Json::from(o.name.clone())),
                ("source", source(&o.source)),
                ("settings", settings(&o.settings)),
                ("metadata", settings(&o.metadata)),
                (
                    "parts",
                    Json::Array(
                        o.parts
                            .iter()
                            .map(|p| {
                                Json::object([
                                    ("name", Json::from(p.name.clone())),
                                    ("source", source(&p.source)),
                                    (
                                        "kind",
                                        Json::from(super::write::kind(&p.kind, Flavour::Prusa)),
                                    ),
                                    ("settings", settings(&p.settings)),
                                    ("metadata", settings(&p.metadata)),
                                    ("transform", matrix(p.transform)),
                                    ("vertices", Json::number(p.mesh.vertices.len())),
                                    ("triangles", Json::number(p.mesh.faces.len())),
                                    ("negative_winding", Json::number(p.mesh.volume() < 0.)),
                                    (
                                        "painting_reversed",
                                        Json::Array(
                                            PaintKind::ALL
                                                .into_iter()
                                                .map(|k| Json::number(p.painting.get(k).reversed()))
                                                .collect(),
                                        ),
                                    ),
                                ])
                            })
                            .collect(),
                    ),
                ),
                (
                    "ranges",
                    Json::Array(
                        o.height_ranges
                            .iter()
                            .map(|r| {
                                Json::object([
                                    ("min_z", Json::number(r.min_z)),
                                    ("max_z", Json::number(r.max_z)),
                                    ("settings", settings(&r.settings)),
                                ])
                            })
                            .collect(),
                    ),
                ),
                (
                    "instances",
                    Json::Array(
                        o.instances
                            .iter()
                            .map(|i| {
                                Json::object([
                                    ("transform", matrix(i.transform)),
                                    ("printable", Json::number(i.printable)),
                                    (
                                        "plate",
                                        i.plate.map_or(Json::Atom("null".into()), Json::number),
                                    ),
                                ])
                            })
                            .collect(),
                    ),
                ),
            ])
        })
        .collect();
    Json::object([
        ("schema", Json::number(1)),
        ("mica_version", Json::from(env!("CARGO_PKG_VERSION"))),
        ("flavour", Json::from(flavour.name())),
        ("native_entries", manifest),
        (
            "config",
            p.config.as_ref().map_or(Json::Atom("null".into()), |c| {
                Json::object([
                    ("path", Json::from(c.path.clone())),
                    ("text", Json::from(c.text.clone())),
                ])
            }),
        ),
        ("objects", Json::Array(objects)),
        (
            "plates",
            Json::Array(
                p.plates
                    .iter()
                    .map(|p| {
                        Json::object([
                            ("id", Json::number(p.id)),
                            ("name", Json::from(p.name.clone())),
                            ("metadata", settings(&p.metadata)),
                            (
                                "instances",
                                Json::Array(
                                    p.instances
                                        .iter()
                                        .map(|i| {
                                            Json::Array(vec![
                                                Json::number(i.object),
                                                Json::number(i.instance),
                                            ])
                                        })
                                        .collect(),
                                ),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "wipe_towers",
            Json::object(p.wipe_towers.iter().map(|(k, v)| {
                (
                    k.to_string(),
                    Json::Array(v.iter().map(Json::number).collect()),
                )
            })),
        ),
        (
            "ignored_features",
            Json::Array(p.ignored_features.iter().cloned().map(Json::from).collect()),
        ),
        ("mica_settings", settings(&p.mica_settings)),
    ])
}
fn read<R: Read + Seek>(zip: &mut zip::ZipArchive<R>) -> Result<Option<Json>, ThreeMfError> {
    let text = {
        let Ok(file) = zip.by_name("Metadata/mica.json") else {
            return Ok(None);
        };
        let mut s = String::new();
        // Metadata is bounded independently of mesh size, including malicious archives.
        file.take(64 * 1024 * 1024 + 1).read_to_string(&mut s)?;
        if s.len() > 64 * 1024 * 1024 {
            return Err(invalid("Mica metadata exceeds 64 MiB"));
        }
        s
    };
    let j = json::parse(&text)?;
    if j.get("schema")?.num::<u32>()? != 1 {
        return Err(invalid("unsupported Mica metadata schema"));
    }
    Ok(Some(j))
}

/// The effective original config used by a full project import, without loading geometry.
/// The outer Option distinguishes no/stale marker from a valid marker with no config.
pub(super) fn config<R: Read + Seek>(
    zip: &mut zip::ZipArchive<R>,
) -> Result<Option<Option<RawConfig>>, ThreeMfError> {
    let Some(j) = read(zip)? else {
        return Ok(None);
    };
    if j.get("native_entries")? != &manifest(zip)? {
        return Ok(None);
    }
    Ok(Some(read_config(j.get("config")?)?))
}
/// `mica_settings` of a current marker (older markers have none).
pub(super) fn mica_settings<R: Read + Seek>(
    zip: &mut zip::ZipArchive<R>,
) -> Result<Settings, ThreeMfError> {
    let Some(j) = read(zip)? else {
        return Ok(Settings::new());
    };
    if j.get("native_entries")? != &manifest(zip)? {
        return Ok(Settings::new());
    }
    j.get("mica_settings")
        .map_or(Ok(Settings::new()), read_settings)
}
fn read_config(c: &Json) -> Result<Option<RawConfig>, ThreeMfError> {
    Ok(if matches!(c, Json::Atom(s) if s == "null") {
        None
    } else {
        Some(RawConfig {
            path: c.get("path")?.text()?.into(),
            text: c.get("text")?.text()?.into(),
        })
    })
}

pub(super) fn restore<R: Read + Seek>(
    zip: &mut zip::ZipArchive<R>,
    p: &mut Project,
) -> Result<(), ThreeMfError> {
    let Some(j) = read(zip)? else {
        return Ok(());
    };
    if j.get("native_entries")? != &manifest(zip)? {
        p.ignored_features
            .insert("stale Mica metadata (native archive changed)".into());
        return Ok(());
    }
    let objects = j.get("objects")?.array()?;
    if objects.len() != p.objects.len() {
        return Err(invalid("Mica object count mismatch"));
    }
    for (o, j) in p.objects.iter_mut().zip(objects) {
        let parts = j.get("parts")?.array()?;
        if parts.len() != o.parts.len() {
            return Err(invalid("Mica part count mismatch"));
        }
        o.name = j.get("name")?.text()?.into();
        o.source = read_source(j.get("source")?)?;
        o.settings = read_settings(j.get("settings")?)?;
        o.metadata = read_settings(j.get("metadata")?)?;
        for (p, j) in o.parts.iter_mut().zip(parts) {
            if j.get("triangles")?.num::<usize>()? != p.mesh.faces.len() {
                return Err(invalid("Mica triangle count mismatch"));
            }
            p.name = j.get("name")?.text()?.into();
            p.source = read_source(j.get("source")?)?;
            p.settings = read_settings(j.get("settings")?)?;
            p.metadata = read_settings(j.get("metadata")?)?;
            p.transform = read_matrix(j.get("transform")?)?;
            p.kind = super::metadata::kind(&Settings::from([(
                "volume_type".into(),
                j.get("kind")?.text()?.into(),
            )]));
            if j.get("negative_winding")?.num::<bool>()? != (p.mesh.volume() < 0.) {
                for f in &mut p.mesh.faces {
                    f.swap(1, 2);
                }
            }
            let flags = j.get("painting_reversed")?.array()?;
            // Projects from before texture paint carry four flags.
            if !(4..=PaintKind::ALL.len()).contains(&flags.len()) {
                return Err(invalid("Mica painting flags length"));
            }
            for (k, flag) in PaintKind::ALL.into_iter().zip(flags) {
                let states = p.painting.get_mut(k);
                *states = std::mem::take(states).with_reversed(flag.num()?);
            }
        }
        o.height_ranges = j
            .get("ranges")?
            .array()?
            .iter()
            .map(|j| {
                Ok(HeightRange {
                    min_z: j.get("min_z")?.num()?,
                    max_z: j.get("max_z")?.num()?,
                    settings: read_settings(j.get("settings")?)?,
                })
            })
            .collect::<Result<_, ThreeMfError>>()?;
        let instances = j.get("instances")?.array()?;
        if instances.len() != o.instances.len() {
            return Err(invalid("Mica instance count mismatch"));
        }
        for (i, j) in o.instances.iter_mut().zip(instances) {
            i.transform = read_matrix(j.get("transform")?)?;
            i.printable = j.get("printable")?.num()?;
            i.plate = if j.get("plate")?.text()? == "null" {
                None
            } else {
                Some(j.get("plate")?.num()?)
            };
        }
    }
    p.config = read_config(j.get("config")?)?;
    p.plates = j
        .get("plates")?
        .array()?
        .iter()
        .map(|j| {
            Ok(Plate {
                id: j.get("id")?.num()?,
                name: j.get("name")?.text()?.into(),
                metadata: read_settings(j.get("metadata")?)?,
                instances: j
                    .get("instances")?
                    .array()?
                    .iter()
                    .map(|j| {
                        let a = j.array()?;
                        if a.len() != 2 {
                            return Err(invalid("Mica plate instance length"));
                        }
                        Ok(PlateInstance {
                            object: a[0].num()?,
                            instance: a[1].num()?,
                        })
                    })
                    .collect::<Result<_, ThreeMfError>>()?,
            })
        })
        .collect::<Result<_, ThreeMfError>>()?;
    p.wipe_towers = j
        .get("wipe_towers")?
        .map()?
        .iter()
        .map(|(k, v)| {
            let v = v.array()?;
            if v.len() != 3 {
                return Err(invalid("Mica tower length"));
            }
            Ok((
                k.parse().map_err(|_| invalid("Mica tower index"))?,
                [v[0].num()?, v[1].num()?, v[2].num()?],
            ))
        })
        .collect::<Result<_, ThreeMfError>>()?;
    p.ignored_features = j
        .get("ignored_features")?
        .array()?
        .iter()
        .map(|j| j.text().map(str::to_owned))
        .collect::<Result<_, _>>()?;
    if let Ok(settings) = j.get("mica_settings") {
        p.mica_settings = read_settings(settings)?;
    }
    if let Ok(file) = zip.by_name(super::paint::BACKUP) {
        super::paint::restore(std::io::BufReader::new(file), p)?;
    }
    super::write::validate(p)?;
    Ok(())
}
