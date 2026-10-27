// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Format/3mf.cpp :: _3MF_Importer::_generate_volumes
// Source: orca@8500fcdcca src/libslic3r/Format/bbs_3mf.cpp :: _BBS_3MF_Importer::_generate_volumes_new
// Changes: Flatten recursive components into object-coordinate parts; retain build instances; compact triangle-range meshes with retained f64 source geometry; detect cycles; metadata matrices remain source data.
// Upstream attribution (3mf.cpp):
// Copyright (c) Prusa Research 2018 - 2023 Oleksandra Iushchenko @YuSanka, David Kocík @kocikdav, Enrico Turri @enricoturri1966, Lukáš Matěna @lukasmatena, Lukáš Hejl @hejllukas, Filip Sykala @Jony01, Vojtěch Bubník @bubnikv, Tomáš Mészáros @tamasmeszaros
// Copyright (c) 2020 Henner Zeller
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher
//

use super::metadata::{ObjectMeta, kind, split};
use super::parse::Resource;
use super::{
    Object, Part, PartKind, ResourceId, Settings, ThreeMfError, Transform, append_mesh, invalid,
};
use crate::mesh::TriangleMesh;
use std::collections::BTreeMap;

pub(super) fn object(
    id: &ResourceId,
    root: &str,
    resources: &BTreeMap<ResourceId, Resource>,
    metadata: &BTreeMap<u32, ObjectMeta>,
) -> Result<Object, ThreeMfError> {
    let resource = resources
        .get(id)
        .ok_or_else(|| invalid(format!("missing object {} in {}", id.id, id.path)))?;
    let mut assembly = Assembly {
        root,
        resources,
        metadata,
        visiting: Vec::new(),
        parts: Vec::new(),
    };
    assembly.expand(id, Transform::IDENTITY, &Settings::new())?;
    if assembly.parts.is_empty() {
        return Err(invalid(format!("object {} has no mesh", id.id)));
    }
    // Prusa writes additional instances as single-component aliases without their own config.
    let mut config_id = id;
    let mut config_resource = resource;
    for _ in 0..128 {
        if config_id.path == root && metadata.contains_key(&config_id.id) {
            break;
        }
        if config_resource.components.len() != 1 {
            break;
        }
        config_id = &config_resource.components[0].source;
        config_resource = &resources[config_id];
    }
    let values = if config_id.path == root {
        metadata.get(&config_id.id).map(|m| &m.values)
    } else {
        None
    };
    let name = values
        .and_then(|m| m.get("name"))
        .cloned()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if !resource.name.is_empty() {
                resource.name.clone()
            } else if !assembly.parts[0].name.is_empty() {
                assembly.parts[0].name.clone()
            } else {
                format!("Object {}", id.id)
            }
        });
    for part in &mut assembly.parts {
        if part.name.is_empty() {
            part.name = name.clone();
        }
    }
    let (settings, metadata) = values.map(split).unwrap_or_default();
    Ok(Object {
        height_ranges: Vec::new(),
        layer_height_profile: Vec::new(),
        brim_points: Vec::new(),
        name,
        source: id.clone(),
        parts: assembly.parts,
        instances: Vec::new(),
        settings,
        metadata,
    })
}

struct Assembly<'a> {
    root: &'a str,
    resources: &'a BTreeMap<ResourceId, Resource>,
    metadata: &'a BTreeMap<u32, ObjectMeta>,
    visiting: Vec<ResourceId>,
    parts: Vec<Part>,
}
impl Assembly<'_> {
    fn expand(
        &mut self,
        id: &ResourceId,
        transform: Transform,
        inherited: &Settings,
    ) -> Result<(), ThreeMfError> {
        if self.visiting.contains(id) {
            return Err(invalid(format!("component cycle at {}:{}", id.path, id.id)));
        }
        if self.visiting.len() >= 128 {
            return Err(invalid("component nesting exceeds 128"));
        }
        self.visiting.push(id.clone());
        let r = self
            .resources
            .get(id)
            .ok_or_else(|| invalid(format!("missing object {} in {}", id.id, id.path)))?;
        let meta = if id.path == self.root {
            self.metadata.get(&id.id)
        } else {
            None
        };
        if !r.components.is_empty() {
            for (index, c) in r.components.iter().enumerate() {
                let part_meta = meta.and_then(|m| {
                    m.parts
                        .get(index)
                        .filter(|p| p.id == Some(c.source.id))
                        .or_else(|| m.parts.iter().find(|p| p.id == Some(c.source.id)))
                });
                let mut values = part_meta.map(|p| p.values.clone()).unwrap_or_default();
                values.extend(inherited.clone());
                self.expand(&c.source, transform.compose(c.transform), &values)?;
            }
        } else {
            if r.mesh.faces.is_empty() {
                return Err(invalid(format!("empty mesh {}:{}", id.path, id.id)));
            }
            let ranges: Vec<_> = meta
                .into_iter()
                .flat_map(|m| &m.parts)
                .filter(|p| p.range.is_some())
                .collect();
            if ranges.is_empty() {
                self.part(id, r, transform, inherited, 0, r.mesh.faces.len() - 1)?;
            } else {
                let mut sorted: Vec<_> = ranges.iter().map(|p| p.range.unwrap()).collect();
                sorted.sort_unstable();
                let mut next = 0;
                for (first, last) in sorted {
                    if first != next || last < first || last >= r.mesh.faces.len() {
                        return Err(invalid(
                            "invalid, overlapping or incomplete volume triangle ranges",
                        ));
                    }
                    next = last + 1;
                }
                if next != r.mesh.faces.len() {
                    return Err(invalid("volume triangle ranges omit faces"));
                }
                for p in ranges {
                    let mut values = p.values.clone();
                    values.extend(inherited.clone());
                    let (first, last) = p.range.unwrap();
                    self.part(id, r, transform, &values, first, last)?;
                }
            }
        }
        self.visiting.pop();
        Ok(())
    }

    fn part(
        &mut self,
        id: &ResourceId,
        r: &Resource,
        transform: Transform,
        values: &Settings,
        first: usize,
        last: usize,
    ) -> Result<(), ThreeMfError> {
        if self.parts.len() >= 100_000 {
            return Err(invalid("too many expanded parts"));
        }
        let mut mesh = TriangleMesh::default();
        let mut vertices = Vec::new();
        let mut painting = r.painting.triangle_range(first, last);
        let mut remap = vec![u32::MAX; r.mesh.vertices.len()];
        for face in &r.mesh.faces[first..=last] {
            let mut output = [0; 3];
            for (slot, &v) in output.iter_mut().zip(face) {
                let index = &mut remap[v as usize];
                if *index == u32::MAX {
                    *index = mesh.vertices.len() as u32;
                    mesh.vertices.push(r.mesh.vertices[v as usize]);
                    vertices.push(r.vertices[v as usize]);
                }
                *slot = *index;
            }
            mesh.faces.push(output);
        }
        // Normalize individual Prusa ranges as well as whole resource meshes.
        if mesh.volume() < 0.0 {
            painting.reverse_winding();
            for face in &mut mesh.faces {
                face.swap(1, 2);
            }
        }
        let mut placed = TriangleMesh::default();
        append_mesh(&mut placed, &mesh, transform)?;
        if transform.determinant() < 0. {
            painting.reverse_winding();
        }
        let (settings, metadata) = split(values);
        self.parts.push(Part {
            name: values
                .get("name")
                .cloned()
                .unwrap_or_else(|| r.name.clone()),
            // Bambu Studio writes the meshes of negative parts and modifiers as objects of
            // type "other"; like Orca, their kind comes from the part's settings.
            kind: if r.core_type == "model" || r.core_type == "other" {
                kind(values)
            } else {
                PartKind::Unknown(r.core_type.clone())
            },
            source: id.clone(),
            source_mesh: Some(std::sync::Arc::new(super::SourceMesh {
                vertices,
                faces: mesh.faces,
                transform,
            })),
            mesh: placed,
            painting,
            transform,
            settings,
            metadata,
        });
        Ok(())
    }
}
