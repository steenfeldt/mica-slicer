// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/Format/bbs_3mf.cpp :: _BBS_3MF_Exporter
// Source: prusaslicer@b028299c77 src/libslic3r/Format/3mf.cpp :: _3MF_Exporter
// Source: snapmaker-orca@b1831e5dcb src/libslic3r/Format/bbs_3mf.cpp :: _BBS_3MF_Importer::_handle_end_metadata
// Changes: Bounded buffered XML into zip; retained f64 Orca source meshes/components and merged Prusa object-coordinate parts, native painting and ranges, atomic replacement and geometry-free Mica sidecar; embossed text/SVG sources as `<slic3rpe:text>` / `<slic3rpe:shape>` part elements with the SVG file in the archive (TextConfigurationSerialization::to_xml, to_xml(EmbossShape)); no cached slices.
// Source: orca@8500fcdcca src/libslic3r/Format/bbs_3mf.cpp :: _BBS_3MF_Exporter::_add_relationships_file_to_archive, _BBS_3MF_Exporter::_add_content_types_file_to_archive, _BBS_3MF_Exporter::_add_model_config_file_to_archive
// Source: prusaslicer@b028299c77 src/libslic3r/Format/3mf.cpp :: _3MF_Exporter::_add_relationships_file_to_archive, _3MF_Exporter::_add_thumbnail_file_to_archive
// Changes: Project pictures (`thumbnails.rs`): the package thumbnail `Metadata/thumbnail.png` and its
// Changes: OPC relationship in both flavours (Orca points it at plate 1's picture; Mica writes that
// Changes: picture under the standard name too), Bambu's cover-middle/-small relationships, the `png`
// Changes: content type, per-plate `thumbnail_file` / `thumbnail_no_light_file`; PNGs stored
// Changes: uncompressed as Orca adds them, drawn on another thread while the geometry streams.
// Upstream attribution (3mf.cpp): Copyright (c) Prusa Research 2018 - 2023
// Oleksandra Iushchenko, David Kocík, Enrico Turri, Lukáš Matěna, Lukáš Hejl,
// Filip Sykala, Vojtěch Bubník, Tomáš Mészáros; Copyright (c) 2020 Henner Zeller.
// PrusaSlicer is released under the terms of the AGPLv3 or higher.

use super::*;
use crate::painting::{FacetStates, PaintKind};
use std::{
    fs::OpenOptions,
    io::{BufWriter, Write},
    sync::atomic::{AtomicU64, Ordering},
};
use zip::{ZipWriter, write::SimpleFileOptions};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Flavour {
    #[default]
    Orca,
    Prusa,
}
impl Flavour {
    pub fn name(self) -> &'static str {
        match self {
            Self::Orca => "orca",
            Self::Prusa => "prusa",
        }
    }
}

/// Save a project atomically. XML is streamed with a 64 KiB buffer, without a second
/// mesh or full XML document. Orca uses retained source vertices/components when the
/// editable mesh still matches; Prusa uses merged object-coordinate parts. Inspection
/// transforms are never applied twice. Settings in the native
/// dialect are retained verbatim; cross-dialect equivalents are translated conservatively.
pub fn write(
    path: impl AsRef<Path>,
    project: &Project,
    flavour: Flavour,
) -> Result<(), ThreeMfError> {
    validate(project)?;
    let path = path.as_ref();
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let (tmp, file) = loop {
        let tmp = dir.join(format!(
            ".mica-3mf-{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match OpenOptions::new()
            .write(true)
            .read(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(f) => break (tmp, f),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    };
    let result = (|| {
        archive(file, project, flavour)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}
fn entry<W: Write + Seek>(
    zip: &mut ZipWriter<W>,
    path: &str,
    f: impl FnOnce(&mut BufWriter<&mut ZipWriter<W>>) -> Result<(), ThreeMfError>,
) -> Result<(), ThreeMfError> {
    zip.start_file(
        path,
        SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .large_file(true),
    )?;
    let mut w = BufWriter::with_capacity(64 * 1024, zip);
    f(&mut w)?;
    w.flush()?;
    Ok(())
}
/// The OPC package thumbnail relationship (Finder, Quick Look, Printables, both slicers).
const THUMBNAIL_REL: &str =
    "http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail";
const MODEL_REL: &str = "http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel";

/// Writes the archive. The pictures (`thumbnails::pictures`) are drawn on another thread
/// while the geometry streams out, and are added once both are done.
fn archive(file: std::fs::File, p: &Project, flavour: Flavour) -> Result<(), ThreeMfError> {
    let config = super::write_config::config(p, flavour)?;
    std::thread::scope(|s| {
        let pictures = s.spawn(|| super::thumbnails::pictures(p, flavour, &config));
        let pictures = || {
            pictures.join().unwrap_or_else(|_| {
                Err(invalid(
                    "drawing the project thumbnails failed unexpectedly",
                ))
            })
        };
        archive_with(file, p, flavour, &config, pictures)
    })
}
fn archive_with(
    file: std::fs::File,
    p: &Project,
    flavour: Flavour,
    config: &str,
    pictures: impl FnOnce() -> Result<Vec<(String, Vec<u8>)>, ThreeMfError>,
) -> Result<(), ThreeMfError> {
    let mut zip = ZipWriter::new(file);
    entry(&mut zip, "[Content_Types].xml", |w| {
        w.write_all(br#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/><Default Extension="png" ContentType="image/png"/><Default Extension="config" ContentType="text/plain"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="json" ContentType="application/json"/><Default Extension="bin" ContentType="application/octet-stream"/></Types>"#)?;
        Ok(())
    })?;
    entry(&mut zip, "_rels/.rels", |w| {
        // As `_add_relationships_file_to_archive`: the model, the package thumbnail and, for
        // Orca, Bambu's cover pictures (plate 1's, middle and small).
        let mut rels = vec![
            ("/3D/3dmodel.model".to_string(), MODEL_REL),
            (format!("/{}", super::thumbnails::PACKAGE), THUMBNAIL_REL),
        ];
        if flavour == Flavour::Orca {
            rels.push((
                format!("/{}", super::thumbnails::orca_plate(1)),
                "http://schemas.bambulab.com/package/2021/cover-thumbnail-middle",
            ));
            rels.push((
                format!("/{}", super::thumbnails::orca_small(1)),
                "http://schemas.bambulab.com/package/2021/cover-thumbnail-small",
            ));
        }
        relationships(w, &rels)
    })?;
    if let Some(text) = &p.full_spectrum_json {
        entry(&mut zip, crate::full_spectrum::ARCHIVE_PATH, |w| {
            w.write_all(text.as_bytes())?;
            Ok(())
        })?;
    }
    let ids: Vec<u32> = p
        .objects
        .iter()
        .scan(1u32, |n, o| {
            let id = *n;
            *n += if flavour == Flavour::Prusa {
                o.instances.len() as u32
            } else {
                1
            };
            Some(id)
        })
        .collect();
    // Orca's shared-mesh bookkeeping also keys on numeric volume IDs: keep them
    // globally unique even though the 3MF Production extension scopes IDs by path.
    let part_ids: Vec<u32> = p
        .objects
        .iter()
        .scan(p.objects.len() as u32 + 1, |next, o| {
            let first = *next;
            *next += o.parts.len() as u32;
            Some(first)
        })
        .collect();
    entry(&mut zip, "3D/3dmodel.model", |w| {
        header(w, flavour)?;
        writeln!(w, "<resources>")?;
        for (oi, o) in p.objects.iter().enumerate() {
            let id = ids[oi];
            writeln!(
                w,
                "<object id=\"{id}\" type=\"model\" name=\"{}\">",
                xml(&o.name)
            )?;
            if flavour == Flavour::Orca {
                writeln!(w, "<components>")?;
                for (pi, part) in o.parts.iter().enumerate() {
                    write!(
                        w,
                        "<component objectid=\"{}\" p:path=\"/3D/Objects/object_{}.model\" transform=\"",
                        part_ids[oi] + pi as u32,
                        oi + 1
                    )?;
                    transform(
                        w,
                        source_mesh(part).map_or(Transform::IDENTITY, |s| s.transform),
                    )?;
                    writeln!(w, "\"/>")?;
                }
                writeln!(w, "</components>")?;
            } else {
                mesh(w, &o.parts, flavour)?;
            }
            writeln!(w, "</object>")?;
            if flavour == Flavour::Prusa {
                for ii in 1..o.instances.len() {
                    writeln!(
                        w,
                        "<object id=\"{}\" type=\"model\"><components><component objectid=\"{id}\"/></components></object>",
                        id + ii as u32
                    )?;
                }
            }
        }
        writeln!(w, "</resources><build>")?;
        for (oi, o) in p.objects.iter().enumerate() {
            for (ii, i) in o.instances.iter().enumerate() {
                let id = ids[oi]
                    + if flavour == Flavour::Prusa {
                        ii as u32
                    } else {
                        0
                    };
                write!(
                    w,
                    "<item objectid=\"{id}\" printable=\"{}\" transform=\"",
                    u8::from(
                        i.printable
                            && (flavour == Flavour::Orca
                                || p.plates.is_empty()
                                || i.plate == p.plates.first().map(|p| p.id))
                    )
                )?;
                transform(w, i.transform)?;
                writeln!(w, "\"/>")?;
            }
        }
        writeln!(w, "</build></model>")?;
        Ok(())
    })?;
    if flavour == Flavour::Orca {
        let paths: Vec<_> = (1..=p.objects.len())
            .map(|i| (format!("/3D/Objects/object_{i}.model"), MODEL_REL))
            .collect();
        entry(&mut zip, "3D/_rels/3dmodel.model.rels", |w| {
            relationships(w, &paths)
        })?;
        for (i, o) in p.objects.iter().enumerate() {
            entry(
                &mut zip,
                &format!("3D/Objects/object_{}.model", i + 1),
                |w| {
                    header(w, flavour)?;
                    writeln!(w, "<resources>")?;
                    for (pi, part) in o.parts.iter().enumerate() {
                        writeln!(
                            w,
                            "<object id=\"{}\" type=\"model\" name=\"{}\">",
                            part_ids[i] + pi as u32,
                            xml(&part.name)
                        )?;
                        mesh(w, std::slice::from_ref(part), flavour)?;
                        writeln!(w, "</object>")?;
                    }
                    writeln!(w, "</resources><build/></model>")?;
                    Ok(())
                },
            )?;
        }
    }
    let config_path = if flavour == Flavour::Orca {
        "Metadata/model_settings.config"
    } else {
        "Metadata/Slic3r_PE_model.config"
    };
    entry(&mut zip, config_path, |w| {
        model_config(w, p, flavour, &ids, &part_ids)
    })?;
    entry(
        &mut zip,
        if flavour == Flavour::Orca {
            "Metadata/project_settings.config"
        } else {
            "Metadata/Slic3r_PE.config"
        },
        |w| {
            w.write_all(config.as_bytes())?;
            Ok(())
        },
    )?;
    if p.objects.iter().any(|o| !o.height_ranges.is_empty()) {
        entry(
            &mut zip,
            if flavour == Flavour::Orca {
                "Metadata/layer_config_ranges.xml"
            } else {
                "Metadata/Prusa_Slicer_layer_config_ranges.xml"
            },
            |w| {
                writeln!(w, "<?xml version=\"1.0\" encoding=\"UTF-8\"?><objects>")?;
                for (i, o) in p
                    .objects
                    .iter()
                    .enumerate()
                    .filter(|(_, o)| !o.height_ranges.is_empty())
                {
                    writeln!(w, "<object id=\"{}\">", i + 1)?;
                    for r in &o.height_ranges {
                        writeln!(w, "<range min_z=\"{}\" max_z=\"{}\">", r.min_z, r.max_z)?;
                        for (k, v) in super::write_config::patch(
                            &r.settings,
                            super::write_config::source(p),
                            flavour,
                        ) {
                            writeln!(w, "<option opt_key=\"{}\">{}</option>", xml(&k), xml(&v))?;
                        }
                        writeln!(w, "</range>")?;
                    }
                    writeln!(w, "</object>")?;
                }
                writeln!(w, "</objects>")?;
                Ok(())
            },
        )?;
    }
    if let Some(text) = super::layer_heights::write_profiles(
        p.objects
            .iter()
            .enumerate()
            .map(|(i, o)| (i + 1, o.layer_height_profile.as_slice())),
    ) {
        let path = if flavour == Flavour::Orca {
            super::layer_heights::ORCA_PROFILE
        } else {
            super::layer_heights::PRUSA_PROFILE
        };
        entry(&mut zip, path, |w| Ok(w.write_all(text.as_bytes())?))?;
    }
    // PrusaSlicer has no brim ears (nor a painted brim type): a Prusa-flavoured save drops them.
    if flavour == Flavour::Orca
        && let Some(text) = super::layer_heights::write_brim_points(
            p.objects
                .iter()
                .enumerate()
                .map(|(i, o)| (i + 1, o.brim_points.as_slice())),
        )
    {
        entry(&mut zip, super::layer_heights::BRIM_POINTS, |w| {
            Ok(w.write_all(text.as_bytes())?)
        })?;
    }
    if flavour == Flavour::Prusa
        && let Some(v) = p.wipe_towers.get(&0)
    {
        entry(
            &mut zip,
            "Metadata/Prusa_Slicer_wipe_tower_information.xml",
            |w| {
                writeln!(
                    w,
                    "<?xml version=\"1.0\"?><wipe_tower_information bed_idx=\"0\" position_x=\"{}\" position_y=\"{}\" rotation_deg=\"{}\"/>",
                    v[0], v[1], v[2]
                )?;
                Ok(())
            },
        )?;
    }
    if p.custom_gcodes.values().any(|i| !i.items.is_empty()) {
        entry(
            &mut zip,
            if flavour == Flavour::Orca {
                super::custom_gcode::ORCA_FILE
            } else {
                super::custom_gcode::PRUSA_FILE
            },
            |w| {
                if flavour == Flavour::Orca {
                    super::custom_gcode::write_orca(
                        w,
                        &p.custom_gcodes,
                        &p.custom_gcode_templates,
                    )?;
                } else {
                    super::custom_gcode::write_prusa(
                        w,
                        &p.custom_gcodes,
                        &p.custom_gcode_templates,
                    )?;
                }
                Ok(())
            },
        )?;
    }
    for (path, data) in emboss_files(p) {
        entry(&mut zip, &path, |w| {
            w.write_all(data.as_bytes())?;
            Ok(())
        })?;
    }
    for hash in texture_images(p) {
        if let Some(png) = crate::texture::custom_png(&hash) {
            entry(&mut zip, &crate::texture::archive_path(&hash), |w| {
                w.write_all(&png)?;
                Ok(())
            })?;
        }
    }
    if super::paint::needs_backup(p, flavour) {
        entry(&mut zip, super::paint::BACKUP, |w| {
            super::paint::backup(w, p, flavour)
        })?;
    }
    // PNGs are compressed already: stored, as Orca adds them (MZ_NO_COMPRESSION).
    for (path, png) in pictures()? {
        zip.start_file(
            path,
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
        )?;
        zip.write_all(&png)?;
    }
    let mut readable = zip::ZipArchive::new(zip.finish()?)?;
    let manifest = super::marker::manifest(&mut readable)?;
    let mut zip = ZipWriter::new_append(readable.into_inner())?;
    entry(&mut zip, "Metadata/mica.json", |w| {
        super::marker::encode(p, flavour, manifest).write(w)?;
        Ok(())
    })?;
    zip.finish()?.sync_all()?;
    Ok(())
}
fn header(w: &mut impl Write, f: Flavour) -> Result<(), ThreeMfError> {
    writeln!(w, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>")?;
    write!(
        w,
        "<model unit=\"millimeter\" xml:lang=\"en-US\" xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\" "
    )?;
    if f == Flavour::Orca {
        writeln!(
            w,
            "xmlns:BambuStudio=\"http://schemas.bambulab.com/package/2021\" xmlns:p=\"http://schemas.microsoft.com/3dmanufacturing/production/2015/06\" requiredextensions=\"p\">"
        )?;
        writeln!(w, "<metadata name=\"BambuStudio:3mfVersion\">1</metadata>")?;
        // Orca uses this compatibility tag to select its native project importer.
        writeln!(w, "<metadata name=\"OrcaSlicer\">2.4.1</metadata>")?;
    } else {
        writeln!(
            w,
            "xmlns:slic3rpe=\"http://schemas.slic3r.org/3mf/2017/06\">"
        )?;
        writeln!(w, "<metadata name=\"slic3rpe:Version3mf\">1</metadata>")?;
        for k in [
            "FdmSupportsPaintingVersion",
            "SeamPaintingVersion",
            "MmPaintingVersion",
            "FuzzySkinPaintingVersion",
        ] {
            writeln!(w, "<metadata name=\"slic3rpe:{k}\">2</metadata>")?;
        }
    }
    writeln!(
        w,
        "<metadata name=\"Application\">{}</metadata>",
        if f == Flavour::Orca {
            "BambuStudio-2.4.1".to_string()
        } else {
            format!("Mica Slicer {}", env!("CARGO_PKG_VERSION"))
        }
    )?;
    writeln!(
        w,
        "<metadata name=\"Generator\">Mica Slicer {}</metadata>",
        env!("CARGO_PKG_VERSION")
    )?;
    Ok(())
}
fn relationships(w: &mut impl Write, rels: &[(String, &str)]) -> Result<(), ThreeMfError> {
    writeln!(
        w,
        "<?xml version=\"1.0\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">"
    )?;
    for (i, (target, kind)) in rels.iter().enumerate() {
        writeln!(
            w,
            "<Relationship Target=\"{}\" Id=\"rel{i}\" Type=\"{kind}\"/>",
            xml(target)
        )?;
    }
    writeln!(w, "</Relationships>")?;
    Ok(())
}
fn transform(w: &mut impl Write, t: Transform) -> std::io::Result<()> {
    for c in 0..4 {
        for r in 0..3 {
            if c != 0 || r != 0 {
                write!(w, " ")?;
            }
            write!(w, "{}", t.0[r][c])?;
        }
    }
    Ok(())
}
fn source_mesh(p: &Part) -> Option<&super::SourceMesh> {
    p.source_mesh.as_deref().filter(|s| s.matches(&p.mesh))
}
pub(super) fn wire_flip(p: &Part, f: Flavour) -> bool {
    (p.mesh.volume() < 0.)
        ^ (f == Flavour::Orca && source_mesh(p).is_some_and(|s| s.transform.determinant() < 0.))
}
fn mesh(w: &mut impl Write, parts: &[Part], f: Flavour) -> Result<(), ThreeMfError> {
    writeln!(w, "<mesh><vertices>")?;
    for p in parts {
        if let Some(source) = (f == Flavour::Orca).then(|| source_mesh(p)).flatten() {
            for v in &source.vertices {
                writeln!(w, "<vertex x=\"{}\" y=\"{}\" z=\"{}\"/>", v[0], v[1], v[2])?;
            }
        } else {
            for v in &p.mesh.vertices {
                writeln!(w, "<vertex x=\"{}\" y=\"{}\" z=\"{}\"/>", v[0], v[1], v[2])?;
            }
        }
    }
    writeln!(w, "</vertices><triangles>")?;
    let attrs = if f == Flavour::Orca {
        [
            "paint_supports",
            "paint_seam",
            "paint_color",
            "paint_fuzzy_skin",
        ]
    } else {
        [
            "slic3rpe:custom_supports",
            "slic3rpe:custom_seam",
            "slic3rpe:mmu_segmentation",
            "slic3rpe:fuzzy_skin",
        ]
    };
    let mut offset = 0u32;
    for p in parts {
        let mut streams = PaintKind::ALL.map(|k| p.painting.get(k).entries().peekable());
        let flip = wire_flip(p, f);
        let reorient = PaintKind::ALL.map(|k| p.painting.get(k).reversed() != flip);
        for (i, face) in p.mesh.faces.iter().enumerate() {
            let mut face = *face;
            // Keep outward winding: native mesh repair otherwise changes split-side meaning.
            if flip {
                face.swap(1, 2);
            }
            let [a, b, c] = face.map(|v| v + offset);
            write!(w, "<triangle v1=\"{a}\" v2=\"{b}\" v3=\"{c}\"")?;
            for ((stream, attr), reorient) in streams.iter_mut().zip(attrs).zip(reorient) {
                if stream.peek().is_some_and(|(id, _)| *id as usize == i) {
                    let (_, codes) = stream.next().unwrap();
                    let oriented;
                    let codes = if reorient {
                        oriented = super::paint::mirrored(codes)?;
                        &oriented
                    } else {
                        codes
                    };
                    write!(w, " {attr}=\"{}\"", FacetStates::wire_string(codes))?;
                }
            }
            writeln!(w, "/>")?;
        }
        offset += p.mesh.vertices.len() as u32;
    }
    writeln!(w, "</triangles></mesh>")?;
    Ok(())
}
pub(super) fn kind(k: &PartKind, f: Flavour) -> &str {
    match (k, f) {
        (PartKind::Model, Flavour::Orca) => "normal_part",
        (PartKind::Model, _) => "ModelPart",
        (PartKind::Modifier, Flavour::Orca) => "modifier_part",
        (PartKind::Modifier, _) => "ParameterModifier",
        (PartKind::NegativeVolume, Flavour::Orca) => "negative_part",
        (PartKind::NegativeVolume, _) => "NegativeVolume",
        (PartKind::SupportEnforcer, Flavour::Orca) => "support_enforcer",
        (PartKind::SupportEnforcer, _) => "SupportEnforcer",
        (PartKind::SupportBlocker, Flavour::Orca) => "support_blocker",
        (PartKind::SupportBlocker, _) => "SupportBlocker",
        (PartKind::Unknown(s), _) => s,
    }
}
fn metadata(w: &mut impl Write, ty: &str, k: &str, v: &str) -> std::io::Result<()> {
    writeln!(
        w,
        "<metadata type=\"{ty}\" key=\"{}\" value=\"{}\"/>",
        xml(k),
        xml(v)
    )
}
fn model_config(
    w: &mut impl Write,
    p: &Project,
    f: Flavour,
    ids: &[u32],
    part_ids: &[u32],
) -> Result<(), ThreeMfError> {
    writeln!(w, "<?xml version=\"1.0\" encoding=\"UTF-8\"?><config>")?;
    let from = super::write_config::source(p);
    for (i, o) in p.objects.iter().enumerate() {
        writeln!(
            w,
            "<object id=\"{}\" instances_count=\"{}\">",
            ids[i],
            o.instances.len()
        )?;
        metadata(w, "object", "name", &o.name)?;
        for (k, v) in super::write_config::patch(&o.settings, from, f) {
            metadata(w, "object", &k, &v)?;
        }
        let mut first = 0;
        for (pi, part) in o.parts.iter().enumerate() {
            let ty = if f == Flavour::Orca { "part" } else { "volume" };
            if f == Flavour::Orca {
                writeln!(
                    w,
                    "<part id=\"{}\" subtype=\"{}\">",
                    part_ids[i] + pi as u32,
                    xml(kind(&part.kind, f))
                )?;
            } else {
                writeln!(
                    w,
                    "<volume firstid=\"{first}\" lastid=\"{}\">",
                    first + part.mesh.faces.len() - 1
                )?;
            }
            first += part.mesh.faces.len();
            metadata(w, ty, "name", &part.name)?;
            if f == Flavour::Prusa {
                metadata(w, ty, "volume_type", kind(&part.kind, f))?;
            }
            // This matrix is source/reload information, not another placement transform.
            for (k, v) in &part.metadata {
                if k == "matrix" || k.starts_with("source_") {
                    metadata(w, ty, k, v)?;
                }
            }
            for (k, v) in super::write_config::patch(&part.settings, from, f) {
                metadata(w, ty, &k, &v)?;
            }
            emboss_source(w, part)?;
            writeln!(w, "</{ty}>")?;
        }
        writeln!(w, "</object>")?;
    }
    if f == Flavour::Orca {
        for (plate_index, plate) in p.plates.iter().enumerate() {
            writeln!(w, "<plate>")?;
            metadata(w, "plate", "plater_id", &(plate_index + 1).to_string())?;
            metadata(w, "plate", "plater_name", &plate.name)?;
            for (k, v) in &plate.metadata {
                // `mica_` keys are Mica's own; they live in Metadata/mica.json only.
                if !matches!(k.as_str(), "plater_id" | "plater_name")
                    && !k.ends_with("_file")
                    && !k.starts_with("mica_")
                {
                    metadata(w, "plate", k, v)?;
                }
            }
            // The plate's pictures (`_add_model_config_file_to_archive`).
            let n = plate_index + 1;
            metadata(
                w,
                "plate",
                "thumbnail_file",
                &super::thumbnails::orca_plate(n),
            )?;
            metadata(
                w,
                "plate",
                "thumbnail_no_light_file",
                &super::thumbnails::orca_no_light(n),
            )?;
            for i in &plate.instances {
                writeln!(w, "<model_instance>")?;
                metadata(w, "instance", "object_id", &ids[i.object].to_string())?;
                metadata(w, "instance", "instance_id", &i.instance.to_string())?;
                writeln!(w, "</model_instance>")?;
            }
            writeln!(w, "</plate>")?;
        }
    }
    writeln!(w, "</config>")?;
    Ok(())
}
/// A part's editable emboss source as PrusaSlicer's (and Orca's) `<slic3rpe:shape …/>` and
/// `<slic3rpe:text …/>` elements, from its `slic3rpe:shape:` / `slic3rpe:text:` metadata.
fn emboss_source(w: &mut impl Write, part: &Part) -> std::io::Result<()> {
    use crate::emboss::config::{SHAPE_PREFIX, TEXT_PREFIX};
    for (prefix, tag) in [
        (SHAPE_PREFIX, "slic3rpe:shape"),
        (TEXT_PREFIX, "slic3rpe:text"),
    ] {
        let attrs: Vec<(&str, &String)> = part
            .metadata
            .iter()
            .filter_map(|(k, v)| k.strip_prefix(prefix).map(|k| (k, v)))
            .filter(|(k, _)| {
                !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            })
            .collect();
        if attrs.is_empty() {
            continue;
        }
        write!(w, "<{tag}")?;
        for (k, v) in attrs {
            write!(w, " {k}=\"{}\"", xml(v))?;
        }
        writeln!(w, "/>")?;
    }
    Ok(())
}

/// The SVG files of embossed shapes, at the archive paths their parts name.
fn emboss_files(p: &Project) -> Vec<(String, &str)> {
    use crate::emboss::config::{SHAPE_PREFIX, SVG_DATA_KEY};
    let key = format!("{SHAPE_PREFIX}filepath3mf");
    let mut out: Vec<(String, &str)> = Vec::new();
    for part in p.objects.iter().flat_map(|o| &o.parts) {
        if let (Some(path), Some(data)) = (part.metadata.get(&key), part.metadata.get(SVG_DATA_KEY))
        {
            let path = path.trim_start_matches('/').to_string();
            let reserved = path.starts_with("Metadata/")
                || path.starts_with("_rels/")
                || path.ends_with(".model")
                || path.ends_with(".rels")
                || path == "[Content_Types].xml";
            if !path.is_empty() && !reserved && !out.iter().any(|(p, _)| *p == path) {
                out.push((path, data));
            }
        }
    }
    out
}

/// Custom texture images the project's settings name (`sha256:<hex>` values anywhere in the
/// project, object, part and range settings), sorted.
fn texture_images(p: &Project) -> Vec<String> {
    let mut texts: Vec<&str> = Vec::new();
    if let Some(c) = &p.config {
        texts.push(&c.text);
    }
    texts.extend(p.mica_settings.values().map(String::as_str));
    for o in &p.objects {
        texts.extend(o.settings.values().map(String::as_str));
        for part in &o.parts {
            texts.extend(part.settings.values().map(String::as_str));
        }
        for r in &o.height_ranges {
            texts.extend(r.settings.values().map(String::as_str));
        }
    }
    let mut out = Vec::new();
    for t in texts {
        let mut rest = t;
        while let Some(i) = rest.find("sha256:") {
            let hex: String = rest[i + 7..]
                .chars()
                .take_while(char::is_ascii_hexdigit)
                .collect();
            if hex.len() == 64 {
                out.push(hex.to_ascii_lowercase());
            }
            rest = &rest[i + 7..];
        }
    }
    out.sort();
    out.dedup();
    out
}

fn xml(s: &str) -> String {
    quick_xml::escape::escape(s)
        .replace('\n', "&#10;")
        .replace('\r', "&#13;")
        .replace('\t', "&#9;")
}
pub(super) fn validate(p: &Project) -> Result<(), ThreeMfError> {
    if p.objects.is_empty() {
        return Err(invalid("cannot save a project without objects"));
    }
    let mut ids = BTreeSet::new();
    let mut members = BTreeSet::new();
    for plate in &p.plates {
        valid_text(&plate.name)?;
        valid_settings(&plate.metadata)?;
        if plate.id == 0 || !ids.insert(plate.id) {
            return Err(invalid("invalid or duplicate plate ID"));
        }
        for i in &plate.instances {
            let inst = p
                .objects
                .get(i.object)
                .and_then(|o| o.instances.get(i.instance))
                .ok_or_else(|| invalid("plate instance out of bounds"))?;
            if inst.plate != Some(plate.id) || !members.insert((i.object, i.instance)) {
                return Err(invalid("inconsistent plate membership"));
            }
        }
    }
    let mut resources = 0usize;
    for (oi, o) in p.objects.iter().enumerate() {
        valid_text(&o.name)?;
        valid_settings(&o.settings)?;
        valid_settings(&o.metadata)?;
        resources = resources
            .checked_add(o.instances.len() + o.parts.len())
            .ok_or_else(|| invalid("too many resources"))?;
        if o.parts.is_empty() || o.instances.is_empty() {
            return Err(invalid("objects need parts and instances"));
        }
        if o.parts.len() > 100_000 {
            return Err(invalid("too many parts"));
        }
        let mut vertex_count = 0usize;
        let mut face_count = 0usize;
        for part in &o.parts {
            valid_text(&part.name)?;
            valid_text(kind(&part.kind, Flavour::Prusa))?;
            valid_settings(&part.settings)?;
            valid_settings(&part.metadata)?;
            vertex_count = vertex_count
                .checked_add(part.mesh.vertices.len())
                .ok_or_else(|| invalid("too many vertices"))?;
            face_count = face_count
                .checked_add(part.mesh.faces.len())
                .ok_or_else(|| invalid("too many triangles"))?;
            if face_count > u32::MAX as usize {
                return Err(invalid("too many triangles"));
            }
            if vertex_count > u32::MAX as usize || part.mesh.faces.is_empty() {
                return Err(invalid("empty or oversized mesh"));
            }
            if part.mesh.vertices.iter().flatten().any(|v| !v.is_finite())
                || part
                    .mesh
                    .faces
                    .iter()
                    .flatten()
                    .any(|&i| i as usize >= part.mesh.vertices.len())
            {
                return Err(invalid("invalid mesh coordinates or indices"));
            }
            for k in PaintKind::ALL {
                part.painting
                    .get(k)
                    .validate_mesh(&part.mesh)
                    .map_err(invalid)?;
            }
            valid_transform(part.transform)?;
        }
        for (ii, i) in o.instances.iter().enumerate() {
            valid_transform(i.transform)?;
            if i.plate.is_some() && !members.contains(&(oi, ii)) {
                return Err(invalid("instance missing from its plate"));
            }
        }
        let mut ranges: Vec<_> = o.height_ranges.iter().collect();
        for range in &ranges {
            valid_settings(&range.settings)?;
        }
        ranges.sort_by(|a, b| a.min_z.total_cmp(&b.min_z));
        if ranges.iter().any(|r| {
            !r.min_z.is_finite() || !r.max_z.is_finite() || r.min_z < 0. || r.min_z >= r.max_z
        }) || ranges.windows(2).any(|r| r[0].max_z > r[1].min_z)
        {
            return Err(invalid("invalid or overlapping height ranges"));
        }
    }
    if resources >= u32::MAX as usize {
        return Err(invalid("too many 3MF resources"));
    }
    if p.wipe_towers.values().flatten().any(|v| !v.is_finite()) {
        return Err(invalid("non-finite tower position"));
    }
    if p.wipe_towers.keys().any(|&i| i >= 100_000) {
        return Err(invalid("tower bed index exceeds 100,000"));
    }
    Ok(())
}
fn valid_settings(s: &Settings) -> Result<(), ThreeMfError> {
    for (k, v) in s {
        valid_text(k)?;
        valid_text(v)?;
    }
    Ok(())
}
fn valid_text(s: &str) -> Result<(), ThreeMfError> {
    if s.chars().any(|c| !matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}')) {
        Err(invalid("text contains a character forbidden in XML 1.0"))
    } else { Ok(()) }
}
fn valid_transform(t: Transform) -> Result<(), ThreeMfError> {
    if t.0.iter().flatten().any(|v| !v.is_finite())
        || !t.determinant().is_finite()
        || t.determinant() == 0.
    {
        Err(invalid("singular or non-finite transform"))
    } else {
        Ok(())
    }
}
