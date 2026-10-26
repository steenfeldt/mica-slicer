// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Saving the scene as a 3MF project (File › Save, Save As…): every plate with its objects,
//! instances and transforms, the model parts, modifiers, negative and support volumes, height
//! ranges, facet painting, filament colours and the **effective** settings, written through
//! `mica_core::threemf::write` in OrcaSlicer / Snapmaker or PrusaSlicer flavour. Also the
//! document state the window title and the unsaved-changes prompt read.
//!
//! Settings: a project opened from a native archive whose presets are still in use is saved
//! with its own settings text verbatim (plus any new edits, listed in Orca's
//! `different_settings_to_system`, which Orca honours when reloading a system preset's project
//! copy). Otherwise the selected presets are composed: for Snapmaker printers as Orca's own
//! `project_settings.config` (full per-slot vectors), for Prusa printers as PrusaSlicer's
//! `Slic3r_PE.config`; a Prusa printer saved in Orca flavour (or the other way round) goes
//! through the conservative dialect translation of `mica_core::threemf::translate`.

use crate::{
    app::{App, PlateState},
    overlays::{Overlay, Overlays},
    plates::Plate,
    scene::{self, Object},
};
use eframe::egui;
use mica_core::{
    config::Config,
    painting::FacetPainting,
    print_job::ToolAssignments,
    threemf::{
        self, Flavour, HeightRange as ProjectRange, Instance, Object as ProjectObject, Part,
        PartKind, Plate as ProjectPlate, PlateInstance, Project, RawConfig, ResourceId, Settings,
        Transform,
    },
};
use std::path::{Path, PathBuf};

/// The selected presets, for the modified check and the verbatim-settings rule.
#[derive(Clone, Debug, PartialEq)]
pub struct Presets {
    pub vendor: usize,
    pub printer: String,
    pub process: String,
    pub filaments: Vec<String>,
    pub bed: String,
}

/// What the window title and the unsaved-changes prompt describe: the project file, the
/// flavour it was opened from or saved as, the archive's own settings text (re-saved verbatim
/// while the presets it names are still in use and its edits are still applied) and the state
/// at the last open or save.
#[derive(Default)]
pub struct Document {
    pub path: Option<PathBuf>,
    pub flavour: Option<Flavour>,
    /// The settings text of the opened archive or the last save.
    pub original: Option<RawConfig>,
    /// The presets and overlays `original` describes ("Use project settings", or a save).
    pub applied: Option<(Presets, Overlays)>,
    /// The scene and settings at the last open or save; `None` before anything was opened.
    pub(crate) clean: Option<DocState>,
}

/// Everything that counts as "unsaved changes": the plates with their objects and the
/// settings (presets and edits). The selection does not.
#[derive(Clone, PartialEq)]
pub(crate) struct DocState {
    pub plates: Vec<PlateState>,
    pub overlays: Overlays,
    pub presets: Presets,
    pub mixes: Vec<mica_core::full_spectrum::VirtualExtruder>,
}

/// What waits for the unsaved-changes prompt's answer.
#[derive(Clone, Debug, PartialEq)]
pub enum Pending {
    Close,
    Open(Vec<PathBuf>),
    /// File › New project.
    New,
}

/// The "Save project as" dialog: the file stem, the format and what to do after saving.
pub struct SaveProject {
    pub name: String,
    pub flavour: Flavour,
    pub then: Option<Pending>,
    pub error: Option<String>,
}

pub fn flavour_label(f: Flavour) -> &'static str {
    match f {
        Flavour::Orca => "OrcaSlicer / Snapmaker project (.3mf)",
        Flavour::Prusa => "PrusaSlicer project (.3mf)",
    }
}

fn flavour_hint(f: Flavour) -> &'static str {
    match f {
        Flavour::Orca => {
            "Plates, prime tower positions and settings as OrcaSlicer and Snapmaker Orca store them."
        }
        Flavour::Prusa => {
            "The first plate prints in PrusaSlicer; Mica keeps every plate and the painting."
        }
    }
}

/// Whether any of the files is a project (which replaces the scene) rather than a model.
pub fn is_project(paths: &[PathBuf]) -> bool {
    paths
        .iter()
        .any(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("3mf")))
}

/// A plate's objects as the project saves them: objects sharing an id are instances of one
/// project object (in order of first appearance), each group by instance id. Mica MCP's live
/// mode maps the saved project's objects back to the plate with it.
pub(crate) fn object_groups(plate: &Plate) -> Vec<Vec<usize>> {
    let mut groups: Vec<(usize, Vec<usize>)> = Vec::new();
    for (i, o) in plate.objects.iter().enumerate() {
        match groups.iter_mut().find(|(id, _)| *id == o.object_id) {
            Some((_, members)) => members.push(i),
            None => groups.push((o.object_id, vec![i])),
        }
    }
    groups
        .into_iter()
        .map(|(_, mut members)| {
            members.sort_by_key(|&i| plate.objects[i].instance_id);
            members
        })
        .collect()
}

/// Whether every entry of `inner` is in `outer` with the same value.
fn contains(outer: &Overlays, inner: &Overlays) -> bool {
    let sub = |o: &Overlay, i: &Overlay| i.iter().all(|(k, v)| o.get(k) == Some(v));
    // The project's own values are what the settings text says: still exactly those.
    outer.project == inner.project
        && sub(&outer.print, &inner.print)
        && sub(&outer.printer, &inner.printer)
        && inner
            .filaments
            .iter()
            .enumerate()
            .all(|(t, i)| outer.filaments.get(t).is_some_and(|o| sub(o, i)))
}

/// The entries of `all` that `applied` does not have (new edits since the settings text was
/// established).
fn minus(all: &Overlays, applied: &Overlays) -> Overlays {
    let diff = |a: &Overlay, b: Option<&Overlay>| -> Overlay {
        a.iter()
            .filter(|(k, v)| b.and_then(|b| b.get(*k)) != Some(*v))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    };
    Overlays {
        print: diff(&all.print, Some(&applied.print)),
        printer: diff(&all.printer, Some(&applied.printer)),
        filaments: all
            .filaments
            .iter()
            .enumerate()
            .map(|(t, o)| diff(o, applied.filaments.get(t)))
            .collect(),
        // Colours are written from `App::tool_colours`, whatever the settings text says.
        colours: all.colours.clone(),
        // The settings text holds the project's values already.
        project: Default::default(),
    }
}

#[cfg(test)]
fn is_empty(o: &Overlays) -> bool {
    o.print.is_empty() && o.printer.is_empty() && o.filaments.iter().all(Overlay::is_empty)
}

/// The settings to write over the presets: the edits and the project's own values (Orca
/// takes their values from the effective configuration; only the keys matter).
fn changes(edits: &Overlays) -> mica_profiles::project::Changes {
    let mut out = edits.project.keys();
    out.process.extend(edits.print.clone());
    out.printer.extend(edits.printer.clone());
    out.filaments.resize(
        edits.filaments.len().max(out.filaments.len()),
        Overlay::new(),
    );
    for (tool, f) in edits.filaments.iter().enumerate() {
        out.filaments[tool].extend(f.clone());
    }
    out
}

/// Sparse canonical settings in the flavour's dialect (`orca`: the Orca schema the project is
/// written for, the active Snapmaker bundle's, as `discovery::load_installed` detects it). An
/// Orca import keeps the source
/// spelling of a mapped option next to its canonical value (as an unknown key); the canonical
/// one is what the editor changes, so the stale spelling goes before translating.
///
/// `scope` is the configuration these settings apply on top of (the object's, for a part or
/// range). Orca has no percentages for several PrusaSlicer "mm/s or %" options: those are
/// written as the absolute value they resolve to, so Orca and Mica can read the file back.
fn to_dialect(
    s: &Settings,
    flavour: Flavour,
    orca: mica_profiles::orca::Dialect,
    scope: Option<&Config>,
) -> Settings {
    let mut s = s.clone();
    if flavour == Flavour::Orca
        && let Some(scope) = scope
    {
        let mut c = scope.clone();
        for (k, v) in &s {
            let _ = c.set_raw(k, v);
        }
        mica_profiles::project::orca_absolutes(&mut s, &c, orca);
    }
    let canonical: Vec<String> = s
        .keys()
        .filter(|k| mica_core::config::option_defs().contains_key(k.as_str()))
        .cloned()
        .collect();
    for k in canonical {
        let probe = Settings::from([(k.clone(), String::new())]);
        for orca in threemf::translate(&probe, Flavour::Prusa, Flavour::Orca).into_keys() {
            if orca != k {
                s.remove(&orca);
            }
        }
    }
    threemf::translate(&s, Flavour::Prusa, flavour)
}

/// The object's tool assignments as the metadata keys the loader reads back: `extruder` when
/// the walls and infills share a tool (else one key per role), and the support roles when they
/// differ from what the configuration alone would give.
fn tool_settings(tools: &ToolAssignments, defaults: Option<&ToolAssignments>, s: &mut Settings) {
    let t = tools.0;
    let one = |v: Option<usize>| v.map_or(0, |v| v + 1).to_string();
    match (t[0], t[1], t[2]) {
        (Some(a), Some(b), Some(c)) if a == b && b == c => {
            s.insert("extruder".into(), (a + 1).to_string());
        }
        _ => {
            for (i, key) in [
                "perimeter_extruder",
                "infill_extruder",
                "solid_infill_extruder",
            ]
            .into_iter()
            .enumerate()
            {
                if t[i].is_some() {
                    s.insert(key.into(), one(t[i]));
                }
            }
        }
    }
    for (i, key) in [
        (3, "support_material_extruder"),
        (4, "support_material_interface_extruder"),
    ] {
        if defaults.is_none_or(|d| d.0[i] != t[i]) {
            s.insert(key.into(), one(t[i]));
        }
    }
}

/// One GUI object (the primary instance of a group) as a project object without instances:
/// the model parts with their painting, the editable volumes in the object's frame, the
/// height ranges and the settings in the flavour's dialect.
/// The project object for `o` (and its instances, added by the caller). `frame`: a plain
/// object's model frame ([`Object::model_frame`]), whose mesh as loaded is saved as the model
/// part, in object coordinates, the instances carrying the frame's transforms, so that the
/// CLI (and a reopening editor) slices exactly what the editor slices.
fn convert_object(
    o: &Object,
    index: usize,
    flavour: Flavour,
    orca: mica_profiles::orca::Dialect,
    defaults: Option<&ToolAssignments>,
    global: Option<&Config>,
    frame: Option<&mica_core::input::Frame>,
) -> Result<ProjectObject, String> {
    let scope = global.and_then(|c| crate::worker::object_config(c, o).ok());
    let dialect = |s: &Settings| to_dialect(s, flavour, orca, scope.as_ref());
    let source = ResourceId {
        path: "3D/3dmodel.model".into(),
        id: index as u32 + 1,
    };
    let part = |name: String,
                kind: PartKind,
                mesh: mica_core::mesh::TriangleMesh,
                painting: FacetPainting,
                settings: &Settings| Part {
        name,
        kind,
        source: source.clone(),
        mesh,
        source_mesh: None,
        painting,
        transform: Transform::IDENTITY,
        settings: dialect(settings),
        metadata: Settings::new(),
    };
    let mut parts = Vec::new();
    match &o.region {
        None => parts.push(part(
            o.name.clone(),
            PartKind::Model,
            frame.map_or_else(|| (*o.mesh).clone(), |f| (*f.source).clone()),
            (*o.painting).clone(),
            &Settings::new(),
        )),
        Some(model) => {
            // The merged mesh is the model volumes' concatenation: the painting splits back by
            // face ranges (see `Object::placed_region`).
            let mut first = 0usize;
            let mut n = 0usize;
            for v in model.volumes.iter().filter(|v| v.kind == PartKind::Model) {
                let faces = v.mesh.faces.len();
                if faces == 0 {
                    continue;
                }
                let painting = if o.paints() {
                    o.painting.triangle_range(first, first + faces - 1)
                } else {
                    v.painting.clone()
                };
                first += faces;
                let name = o
                    .parts
                    .get(n)
                    .map(|(name, _)| name.clone())
                    .unwrap_or_else(|| o.name.clone());
                n += 1;
                parts.push(part(
                    name,
                    PartKind::Model,
                    v.mesh.clone(),
                    painting,
                    &scene::overrides_from(&v.config),
                ));
            }
        }
    }
    if parts.is_empty() {
        return Err(format!("{}: the object has no mesh", o.name));
    }
    // A text object's source rides on its (only) model part.
    if let Some(e) = &o.emboss {
        parts[0]
            .metadata
            .extend(crate::emboss::metadata(e, glam::Mat4::IDENTITY));
    }
    let first_volume = parts.len();
    for v in &o.volumes {
        let mut p = part(
            v.name.clone(),
            v.kind.clone(),
            scene::transform_mesh(&v.mesh, v.local_transform()),
            FacetPainting::default(),
            &v.overrides,
        );
        if let Some(e) = &v.emboss {
            p.metadata
                .extend(crate::emboss::metadata(e, v.local_transform()));
        }
        parts.push(p);
    }
    let mut settings = o.overrides.clone();
    tool_settings(&o.tools, defaults, &mut settings);
    let mut sorted = o.clone();
    sorted.normalize_ranges();
    let saved_ranges: Vec<&scene::HeightRange> = sorted
        .height_ranges
        .iter()
        .filter(|r| r.max_z > r.min_z.max(0.))
        .collect();
    let range_groups = crate::tree::range_metadata(&saved_ranges);
    let height_ranges = saved_ranges
        .iter()
        .map(|r| ProjectRange {
            min_z: f64::from(r.min_z.max(0.)),
            max_z: f64::from(r.max_z),
            settings: dialect(&r.overrides),
        })
        .collect();
    preserve_source(o, &mut parts, flavour, frame.is_some())?;
    // The tree's user groups: Mica's metadata only (after `preserve_source`, which takes a
    // retained part's metadata from the source).
    for (p, v) in parts[first_volume..].iter_mut().zip(&o.volumes) {
        p.metadata.remove(crate::tree::GROUP_KEY);
        p.metadata.extend(crate::tree::volume_metadata(v));
    }
    let mut object = ProjectObject {
        height_ranges,
        layer_height_profile: o.layer_profile.clone(),
        brim_points: Vec::new(),
        name: o.name.clone(),
        source,
        parts,
        instances: Vec::new(),
        settings: dialect(&settings),
        metadata: range_groups.into_iter().collect(),
    };
    object.brim_points = brim_points(o, &object, flavour)?;
    Ok(object)
}

/// The ears in the saved object's frame (the parts' frame: the source's for a retained Orca
/// object, see `preserve_source`), relative to its bounding-box centre as Orca reads them.
fn brim_points(
    o: &Object,
    saved: &ProjectObject,
    flavour: Flavour,
) -> Result<Vec<[f32; 4]>, String> {
    if o.brim_ears.is_empty() {
        return Ok(Vec::new());
    }
    let into_object = match (&o.source, flavour) {
        (Some((_, placement)), Flavour::Orca) => placement
            .inverse()
            .ok_or("invalid source build transform")?,
        _ => Transform::IDENTITY,
    };
    let origin = saved.brim_frame_origin();
    Ok(o.brim_ears
        .iter()
        .map(|e| {
            let p = into_object.apply([e[0], e[1], e[2]]);
            [
                (f64::from(p[0]) - origin[0]) as f32,
                (f64::from(p[1]) - origin[1]) as f32,
                (f64::from(p[2]) - origin[2]) as f32,
                e[3],
            ]
        })
        .collect())
}

/// Recover source parts by geometry, independently of painting/settings edits or added
/// modifiers. New/edited parts are unplaced into the original object's frame; its native
/// build matrix can then remain in double precision as well. `framed`: the parts are a plain
/// object's model frame, already in object coordinates; a part that is the source's own keeps
/// the source's metadata.
fn preserve_source(
    o: &Object,
    parts: &mut [Part],
    flavour: Flavour,
    framed: bool,
) -> Result<(), String> {
    let Some((original, placement)) = &o.source else {
        return Ok(());
    };
    if flavour != Flavour::Orca {
        return Ok(());
    }
    if framed {
        for part in parts {
            if let Some(source) = original.parts.iter().find(|p| {
                p.kind == part.kind
                    && p.mesh.vertices == part.mesh.vertices
                    && p.mesh.faces == part.mesh.faces
            }) {
                part.source_mesh = source.source_mesh.clone();
                part.transform = source.transform;
                part.source = source.source.clone();
                part.metadata = source.metadata.clone();
            }
        }
        return Ok(());
    }
    let inverse = placement
        .inverse()
        .ok_or("invalid source build transform")?;
    let mut used = vec![false; original.parts.len()];
    for part in parts {
        // The emboss source written for this save; its frame moves with the mesh below.
        let mut emboss: Settings = part
            .metadata
            .iter()
            .filter(|(k, _)| mica_core::emboss::config::is_emboss_key(k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        crate::emboss::transform_metadata(&mut emboss, inverse);
        let found = original.parts.iter().enumerate().find(|(i, p)| {
            if used[*i] || p.kind != part.kind {
                return false;
            }
            let mut placed = mica_core::mesh::TriangleMesh::default();
            mica_core::input::append(&mut placed, &p.mesh, *placement);
            placed.vertices == part.mesh.vertices && placed.faces == part.mesh.faces
        });
        if let Some((i, source)) = found {
            used[i] = true;
            part.mesh = source.mesh.clone();
            part.source_mesh = source.source_mesh.clone();
            part.transform = source.transform;
            part.source = source.source.clone();
            part.metadata = source.metadata.clone();
        } else {
            let mesh = std::mem::take(&mut part.mesh);
            let mut unplaced = mica_core::mesh::TriangleMesh::default();
            mica_core::input::append(&mut unplaced, &mesh, inverse);
            part.source_mesh = Some(std::sync::Arc::new(threemf::SourceMesh {
                vertices: mesh.vertices.iter().map(|v| v.map(f64::from)).collect(),
                faces: mesh.faces,
                transform: inverse,
            }));
            part.mesh = unplaced;
            part.transform = inverse;
        }
        part.metadata
            .retain(|k, _| !mica_core::emboss::config::is_emboss_key(k));
        part.metadata.extend(emboss);
        if placement.determinant() < 0. {
            part.painting.reverse_winding();
        }
    }
    Ok(())
}

impl App {
    pub(crate) fn presets(&self) -> Presets {
        Presets {
            vendor: self.profiles.vendor,
            printer: self.profiles.printer.clone(),
            process: self.profiles.process.clone(),
            filaments: self.profiles.filaments.clone(),
            bed: self.profiles.bed.clone(),
        }
    }

    pub(crate) fn document_state(&self) -> DocState {
        DocState {
            plates: self.plate_states(),
            overlays: self.overlays.clone(),
            presets: self.presets(),
            mixes: self.mixes.list.clone(),
        }
    }

    /// Marks the current state as saved / just opened.
    pub(crate) fn mark_clean(&mut self) {
        self.document.clean = Some(self.document_state());
    }

    /// Whether the scene or the settings differ from the last open or save.
    pub(crate) fn is_modified(&self) -> bool {
        let Some(clean) = &self.document.clean else {
            return false;
        };
        if clean.presets != self.presets()
            || clean.overlays != self.overlays
            || clean.mixes != self.mixes.list
            || clean.plates.len() != self.plates.len()
        {
            return true;
        }
        !clean
            .plates
            .iter()
            .zip(&self.plates.plates)
            .all(|(s, p)| plate_matches(s, p))
    }

    /// The project's name for the title and the prompts: the file stem, else `Untitled`.
    pub(crate) fn document_name(&self) -> String {
        self.document
            .path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".into())
    }

    /// `• Name — Mica Slicer` while there are unsaved changes, `Name — Mica Slicer` otherwise.
    pub(crate) fn window_title(&self) -> String {
        if let Some(doc) = &self.gcode {
            return format!("{} (read-only) — Mica Slicer", doc.name);
        }
        format!(
            "{}{} — Mica Slicer",
            if self.is_modified() { "• " } else { "" },
            self.document_name()
        )
    }

    /// The flavour Save As offers first: the one the project was opened from or saved as,
    /// else Orca for Snapmaker printers and Prusa for Prusa printers.
    pub(crate) fn default_flavour(&self) -> Flavour {
        self.document.flavour.unwrap_or(if self.profiles.is_orca() {
            Flavour::Orca
        } else {
            Flavour::Prusa
        })
    }

    /// The tower placement a plate slices with, as `plate_config` applies it: the placement
    /// the project came with, under an edit of `wipe_tower_x/y` in the settings, else the
    /// configuration's position. Orca stores one position per plate, so every plate gets one.
    pub(crate) fn effective_tower(&self, plate: &Plate) -> Option<[f64; 3]> {
        let edit = |key: &str| {
            self.overlays
                .print
                .get(key)
                .and_then(|v| v.trim().parse::<f64>().ok())
                .filter(|v| v.is_finite())
        };
        let edits = [
            edit("wipe_tower_x"),
            edit("wipe_tower_y"),
            edit("wipe_tower_rotation_angle"),
        ];
        match plate.tower_placement {
            Some(t) => Some(std::array::from_fn(|i| edits[i].unwrap_or(t[i]))),
            None => {
                let c = self.effective_config()?;
                let number = |key: &str, default: f64| {
                    c.raw(key)
                        .and_then(|v| v.split(',').next()?.trim().parse::<f64>().ok())
                        .filter(|v| v.is_finite())
                        .unwrap_or(default)
                };
                Some([
                    edits[0].unwrap_or_else(|| number("wipe_tower_x", 180.)),
                    edits[1].unwrap_or_else(|| number("wipe_tower_y", 140.)),
                    edits[2].unwrap_or_else(|| number("wipe_tower_rotation_angle", 0.)),
                ])
            }
        }
    }

    /// The whole scene as a project: plates in Orca's grid, objects sharing an id as one
    /// object with several instances, every part kind, ranges, painting and the settings.
    pub(crate) fn build_project(&self, flavour: Flavour) -> Result<Project, String> {
        let size = self.bed.max - self.bed.min;
        let (width, depth) = (f64::from(size.x), f64::from(size.y));
        let count = self.plates.len();
        let defaults = self.effective_config().and_then(|c| {
            ToolAssignments::resolve(
                c,
                &Settings::new(),
                &Settings::new(),
                0,
                c.vector_len("nozzle_diameter").max(1),
            )
            .ok()
        });
        // The Orca schema of the active Snapmaker bundle (Snapmaker Orca's flow-variant set or
        // OrcaSlicer's own), as the CLI detects it; OrcaSlicer's for a Prusa printer.
        let orca = self
            .profiles
            .orca_dialect()
            .unwrap_or(mica_profiles::orca::Dialect::Orca242);
        let mut project = Project::default();
        if let Some(c) = self.effective_config() {
            let text = |key: &str| c.string(key).unwrap_or("").to_string();
            project.custom_gcode_templates = mica_core::threemf::CustomGCodeTemplates {
                color_change: text("color_change_gcode"),
                pause: text("pause_print_gcode"),
                template: text("template_custom_gcode"),
            };
        }
        for (pi, plate) in self.plates.plates.iter().enumerate() {
            let id = pi as u32 + 1;
            let mut metadata = plate.metadata.clone();
            metadata.retain(|k, _| !matches!(k.as_str(), "plater_id" | "plater_name"));
            // The plate's own settings: Orca's plate keys where Orca has them.
            metadata.extend(crate::plate_settings::to_metadata(&plate.overrides));
            metadata.insert(
                "locked".into(),
                if plate.locked { "true" } else { "false" }.into(),
            );
            // Calibration plates keep their test, tool, parameters and reading in Mica's own
            // metadata entry (`Metadata/mica.json`), which other slicers ignore.
            metadata.remove(crate::calibration::PLATE_METADATA_KEY);
            if let Some(job) = &plate.calibration {
                metadata.insert(
                    crate::calibration::PLATE_METADATA_KEY.into(),
                    job.to_metadata(),
                );
            }
            project.plates.push(ProjectPlate {
                id,
                // Unnamed plates store no name (OrcaSlicer's empty `plater_name`), so a reader
                // numbers them by position.
                name: self.plates.stored_name(pi),
                instances: Vec::new(),
                metadata,
            });
            if let Some(t) = self.effective_tower(plate) {
                project.wipe_towers.insert(pi as u32, t);
            }
            if !plate.custom_gcode.is_empty() {
                project
                    .custom_gcodes
                    .insert(pi as u32, plate.custom_gcode.clone());
            }
            let origin = mica_core::input::grid_origin(pi, count, width, depth);
            for members in object_groups(plate) {
                let oi = project.objects.len();
                // Plain instances of one mesh as loaded save that mesh with each instance's
                // model-frame transform: exactly what they slice from.
                let frames: Option<Vec<mica_core::input::Frame>> = members
                    .iter()
                    .map(|&m| {
                        let o = &plate.objects[m];
                        (o.is_plain() && o.emboss.is_none()).then(|| o.model_frame())
                    })
                    .collect::<Option<Vec<_>>>()
                    .filter(|f| {
                        f.iter()
                            .all(|g| std::sync::Arc::ptr_eq(&g.source, &f[0].source))
                    });
                let mut object = convert_object(
                    &plate.objects[members[0]],
                    oi,
                    flavour,
                    orca,
                    defaults.as_ref(),
                    self.effective_config(),
                    frames.as_ref().map(|f| &f[0]),
                )?;
                for (ii, &m) in members.iter().enumerate() {
                    let o = &plate.objects[m];
                    let mut transform = scene::to_threemf(o.transform());
                    if let Some(frames) = &frames {
                        transform = frames[ii].trafo;
                    } else if flavour == Flavour::Orca
                        && let Some((_, original)) = &o.source
                    {
                        transform = transform.compose(*original);
                    }
                    transform.0[0][3] += origin[0];
                    transform.0[1][3] += origin[1];
                    // Objects beside the bed stay unassigned, as Orca keeps them.
                    let (min, max) = o.bounds();
                    let on_plate = min.x < self.bed.max.x
                        && max.x > self.bed.min.x
                        && min.y < self.bed.max.y
                        && max.y > self.bed.min.y;
                    object.instances.push(Instance {
                        transform,
                        printable: true,
                        plate: on_plate.then_some(id),
                    });
                    if on_plate {
                        project.plates[pi].instances.push(PlateInstance {
                            object: oi,
                            instance: ii,
                        });
                    }
                }
                project.objects.push(object);
            }
        }
        if project.objects.is_empty() {
            return Err("Nothing to save yet: the plates are empty".into());
        }
        project.full_spectrum_json = self.mixes_json();
        self.project_settings(&mut project, flavour)?;
        Ok(project)
    }

    /// The project's settings text in the flavour's dialect (see the module notes).
    fn project_settings(&self, project: &mut Project, flavour: Flavour) -> Result<(), String> {
        let config = self.config.as_ref().map_err(|e| e.clone())?;
        let presets = self.presets();
        let verbatim = self
            .document
            .original
            .as_ref()
            .filter(|c| (c.path.ends_with("project_settings.config")) == (flavour == Flavour::Orca))
            .and_then(|original| {
                let (applied, overlays) = self.document.applied.as_ref()?;
                (*applied == presets && contains(&self.overlays, overlays))
                    .then(|| (original.clone(), minus(&self.overlays, overlays)))
            });
        let colours: Vec<String> = self.tool_colour_texts();
        let (original, edits) = verbatim.map_or_else(
            || (None, self.overlays.clone()),
            |(original, edits)| (Some(original), edits),
        );
        mica_profiles::project::SaveSettings {
            config,
            colours: &colours,
            original: original.as_ref(),
            edits: &changes(&edits),
        }
        .write(project, flavour, || {
            self.profiles
                .orca_project_config(&colours)
                .transpose()
                .map(|selected| {
                    selected.map(|(raw, user)| {
                        (
                            raw,
                            mica_profiles::project::Changes {
                                process: user.print,
                                printer: user.printer,
                                filaments: user.filaments,
                            },
                        )
                    })
                })
        })
    }

    /// Writes the project to `path` and makes it the document.
    pub(crate) fn save_project_to(&mut self, path: &Path, flavour: Flavour) -> Result<(), String> {
        let mut path = path.to_path_buf();
        if path.extension().is_none() {
            path.set_extension("3mf");
        }
        let project = self.build_project(flavour)?;
        threemf::write(&path, &project, flavour).map_err(|e| format!("{}: {e}", path.display()))?;
        self.document.path = Some(path.clone());
        self.document.flavour = Some(flavour);
        self.document.original = project.config.clone();
        self.document.applied = Some((self.presets(), self.overlays.clone()));
        self.mixes_saved(project.full_spectrum_json.clone());
        self.mark_clean();
        self.prefs.push_recent(path.clone());
        self.prefs_dirty = true;
        self.status = format!("Saved {}", path.display());
        Ok(())
    }

    /// Whether Save can write the document in place: a known path and flavour, and a file
    /// that is not read-only (a read-only project goes through Save As).
    fn can_save_in_place(&self) -> Option<(PathBuf, Flavour)> {
        let path = self.document.path.clone()?;
        let flavour = self.document.flavour?;
        if std::fs::metadata(&path).is_ok_and(|m| m.permissions().readonly()) {
            return None;
        }
        Some((path, flavour))
    }

    /// ⌘S: saves in place, or asks where (new, read-only or flavour-less projects).
    pub(crate) fn save(&mut self) {
        if self.plates.is_blank() {
            self.status = "Nothing to save yet".into();
            return;
        }
        match self.can_save_in_place() {
            Some((path, flavour)) => {
                if let Err(e) = self.save_project_to(&path, flavour) {
                    self.notify_error("The project was not saved", e);
                }
            }
            None => self.save_as(None),
        }
    }

    /// ⇧⌘S: opens the Save As dialog; `then` runs after a successful save.
    pub(crate) fn save_as(&mut self, then: Option<Pending>) {
        if self.plates.is_blank() {
            self.status = "Nothing to save yet".into();
            if let Some(p) = then {
                self.dialogs.unsaved = Some(p);
            }
            return;
        }
        let read_only = self.document.path.is_some() && self.can_save_in_place().is_none();
        self.dialogs.save_project = Some(SaveProject {
            name: self.document_name(),
            flavour: self.default_flavour(),
            then,
            error: read_only.then(|| "The opened file is read-only; choose a new name.".into()),
        });
    }

    /// Runs what the unsaved-changes prompt was holding back.
    pub(crate) fn proceed(&mut self, pending: Pending, ctx: &egui::Context) {
        match pending {
            Pending::Close => {
                self.closing = true;
                self.save_prefs();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Pending::Open(paths) => self.load_now(paths, ctx),
            Pending::New => self.new_project_now(),
        }
    }

    /// "Save project as": name, format, then the native file picker.
    pub(crate) fn save_project_dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.dialogs.save_project else {
            return;
        };
        let p = crate::theme::of_ctx(ctx);
        let mut action: Option<bool> = None;
        let mut open = true;
        egui::Window::new("Save project as")
            .id(egui::Id::unique("save-project"))
            .open(&mut open)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0., 120.))
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.set_width(440.);
                ui.add_space(6.);
                ui.label(egui::RichText::new("Name").font(crate::theme::medium(12.)));
                let r = ui.add(
                    egui::TextEdit::singleline(&mut dialog.name)
                        .desired_width(f32::INFINITY)
                        .hint_text("Project name"),
                );
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    action = Some(true);
                }
                ui.add_space(8.);
                ui.label(egui::RichText::new("Format").font(crate::theme::medium(12.)));
                for f in [Flavour::Orca, Flavour::Prusa] {
                    let selected = dialog.flavour == f;
                    if ui.radio(selected, flavour_label(f)).clicked() {
                        dialog.flavour = f;
                    }
                    ui.indent(("flavour-hint", f.name()), |ui| {
                        ui.label(
                            egui::RichText::new(flavour_hint(f))
                                .font(crate::theme::sans(11.))
                                .color(p.muted),
                        );
                    });
                }
                ui.add_space(4.);
                ui.label(
                    egui::RichText::new(
                        "Saves every plate with its objects, modifiers, painting and the current settings.",
                    )
                    .font(crate::theme::sans(11.))
                    .color(p.muted),
                );
                if let Some(e) = &dialog.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                ui.add_space(8.);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if crate::panels::widgets::primary_button(ui, "Save…", true).clicked() {
                            action = Some(true);
                        }
                        if crate::panels::widgets::button(ui, "Cancel", true).clicked() {
                            action = Some(false);
                        }
                    });
                });
                ui.add_space(4.);
            });
        if !open {
            action = Some(false);
        }
        match action {
            Some(true) => {
                let name = dialog.name.trim().to_string();
                let flavour = dialog.flavour;
                let then = dialog.then.clone();
                if name.is_empty() {
                    dialog.error = Some("Give the project a name".into());
                    return;
                }
                if self.headless {
                    self.dialogs.save_project = None;
                    self.status =
                        "Saving needs the file dialog; use --save in headless mode".into();
                    return;
                }
                let mut picker = rfd::FileDialog::new()
                    .set_file_name(format!("{name}.3mf"))
                    .add_filter("3MF project", &["3mf"]);
                if let Some(dir) = self.document.path.as_ref().and_then(|p| p.parent()) {
                    picker = picker.set_directory(dir);
                }
                let Some(path) = picker.save_file() else {
                    return;
                };
                match self.save_project_to(&path, flavour) {
                    Ok(()) => {
                        self.dialogs.save_project = None;
                        if let Some(pending) = then {
                            self.proceed(pending, ctx);
                        }
                    }
                    Err(e) => {
                        if let Some(d) = &mut self.dialogs.save_project {
                            d.error = Some(e);
                        }
                    }
                }
            }
            Some(false) => self.dialogs.save_project = None,
            None => {}
        }
    }

    /// "Save changes to X?" before closing or opening another project: Save / Don't save /
    /// Cancel.
    pub(crate) fn unsaved_prompt(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.dialogs.unsaved.clone() else {
            return;
        };
        let p = crate::theme::of_ctx(ctx);
        let name = self.document_name();
        let mut choice: Option<&str> = None;
        egui::Modal::new(egui::Id::unique("unsaved-changes")).show(ctx, |ui| {
            ui.set_width(420.);
            ui.add_space(4.);
            ui.label(
                egui::RichText::new(format!("Save changes to “{name}”?"))
                    .font(crate::theme::semibold(14.))
                    .color(p.text),
            );
            ui.add_space(4.);
            ui.label(
                egui::RichText::new(match pending {
                    Pending::Close => "Your changes will be lost if you close without saving.",
                    Pending::Open(_) => {
                        "Opening another project replaces this one; unsaved changes will be lost."
                    }
                    Pending::New => {
                        "A new project replaces this one; unsaved changes will be lost."
                    }
                })
                .font(crate::theme::sans(12.))
                .color(p.muted_alt),
            );
            ui.add_space(12.);
            ui.horizontal(|ui| {
                if crate::panels::widgets::button(ui, "Don't save", true).clicked() {
                    choice = Some("discard");
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if crate::panels::widgets::primary_button(ui, "Save", true).clicked() {
                        choice = Some("save");
                    }
                    if crate::panels::widgets::button(ui, "Cancel", true).clicked() {
                        choice = Some("cancel");
                    }
                });
            });
            ui.add_space(4.);
        });
        match choice {
            Some("save") => {
                self.dialogs.unsaved = None;
                match self.can_save_in_place() {
                    Some((path, flavour)) => match self.save_project_to(&path, flavour) {
                        Ok(()) => self.proceed(pending, ctx),
                        Err(e) => self.notify_error("The project was not saved", e),
                    },
                    None => self.save_as(Some(pending)),
                }
            }
            Some("discard") => {
                self.dialogs.unsaved = None;
                self.proceed(pending, ctx);
            }
            Some(_) => self.dialogs.unsaved = None,
            None => {}
        }
    }
}

/// Whether a plate still equals its recorded state (no clones: compared field by field).
fn plate_matches(s: &PlateState, p: &Plate) -> bool {
    s.name == p.name
        && s.locked == p.locked
        && s.tower == p.tower_placement
        && s.calibration == p.calibration
        && s.overrides == p.overrides
        && s.metadata == p.metadata
        && s.objects == p.objects
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Options;
    use mica_core::config::Config;
    use mica_profiles::user::Store;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn native_source_survives_gui_settings_paint_and_added_volume() {
        let mut project = threemf::load_3mf(models().join("painted-cube.3mf")).unwrap();
        for part in &mut project.objects[0].parts {
            let source = std::sync::Arc::make_mut(part.source_mesh.as_mut().unwrap());
            for v in &mut source.vertices {
                v[0] += 0.123456789012345;
            }
            source.transform.0[0][0] = 6.5;
            source.transform.0[1][1] = 6.5;
            source.transform.0[2][2] = 6.5;
            part.transform = source.transform;
            part.mesh.vertices = source
                .vertices
                .iter()
                .map(|v| source.transform.apply(v.map(|x| x as f32)))
                .collect();
        }
        project.objects[0].instances[0].transform.0[0][0] = -1.;
        project.objects[0].instances[0].transform.0[0][3] += 0.123456789012345;
        let input = temp("input.3mf");
        threemf::write(&input, &project, Flavour::Orca).unwrap();
        let mut config = Config::defaults();
        config
            .set_raw("nozzle_diameter", "0.4,0.4,0.4,0.4,0.4")
            .unwrap();
        let loaded = mica_core::input::load(
            &[input],
            (125., 110.),
            false,
            true,
            0,
            &Default::default(),
            &config,
            &mica_profiles::project::Mapper::default(),
        )
        .unwrap();
        let mut object = Object::new(loaded.objects.into_iter().next().unwrap());
        object.overrides.insert("perimeters".into(), "4".into());
        // Painting and added modifiers must not bake unchanged model parts.
        object.painting_edited = true;
        object.volumes.push(crate::scene::Volume::new(
            "extra",
            PartKind::Modifier,
            (*object.mesh).clone(),
        ));
        let mut converted = convert_object(
            &object,
            0,
            Flavour::Orca,
            mica_profiles::orca::Dialect::Orca242,
            None,
            None,
            None,
        )
        .unwrap();
        converted.instances.push(Instance {
            transform: scene::to_threemf(object.transform())
                .compose(object.source.as_ref().unwrap().1),
            printable: true,
            plate: None,
        });
        let saved = temp("output.3mf");
        threemf::write(
            &saved,
            &Project {
                objects: vec![converted],
                ..Default::default()
            },
            Flavour::Orca,
        )
        .unwrap();
        let again = threemf::load_3mf(saved).unwrap();
        assert_eq!(
            project.objects[0].instances[0].transform,
            again.objects[0].instances[0].transform
        );
        let original = project.objects[0].parts[0].source_mesh.as_ref().unwrap();
        let retained = again.objects[0].parts[0].source_mesh.as_ref().unwrap();
        assert_eq!(
            original
                .vertices
                .iter()
                .map(|v| v.map(f64::to_bits))
                .collect::<Vec<_>>(),
            retained
                .vertices
                .iter()
                .map(|v| v.map(f64::to_bits))
                .collect::<Vec<_>>()
        );
        assert_eq!(original.transform, retained.transform);
    }

    static NEXT: AtomicU64 = AtomicU64::new(0);
    fn models() -> PathBuf {
        PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models/"))
    }
    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mica-save-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }
    fn app(ctx: &egui::Context) -> App {
        let dir = std::env::temp_dir().join(format!(
            "mica-save-store-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        App::create(
            ctx,
            Options {
                prefs_path: None,
                store: Some(Store::open(dir)),
                files: vec![],
                headless: true,
            },
        )
    }
    fn select_printer(app: &mut App, name: &str) {
        let (vi, _, found) = app
            .profiles
            .all_printers()
            .into_iter()
            .find(|(_, _, n)| n == name)
            .unwrap_or_else(|| panic!("no printer {name}"));
        app.profiles.select_printer(vi, &found);
        app.settings_changed();
    }
    fn xl(app: &mut App) {
        let name = app
            .profiles
            .all_printers()
            .into_iter()
            .map(|(_, _, n)| n)
            .find(|n| {
                n.contains("XL") && n.contains("5T") && n.contains("0.4") && n.contains("Input")
            })
            .expect("XL 5T preset");
        select_printer(app, &name);
    }
    fn open(app: &mut App, file: &str) {
        let result =
            crate::app::load_files(vec![models().join(file)], &app.config, app.bed.center());
        app.finish_load(result);
        assert!(!app.plates.is_blank(), "{}", app.status);
    }
    fn reopen(ctx: &egui::Context, path: &Path, printer: Option<&str>) -> App {
        let mut again = app(ctx);
        if let Some(p) = printer {
            select_printer(&mut again, p);
        }
        let result =
            crate::app::load_files(vec![path.to_path_buf()], &again.config, again.bed.center());
        again.finish_load(result);
        assert!(!again.plates.is_blank(), "{}", again.status);
        again
    }

    /// Objects, transforms (as placed meshes), painting fingerprints, volumes, ranges, tools
    /// and overrides of every plate match.
    fn same_scene(a: &App, b: &App) {
        assert_eq!(a.plates.len(), b.plates.len());
        for (pa, pb) in a.plates.plates.iter().zip(&b.plates.plates) {
            assert_eq!(pa.name, pb.name);
            assert_eq!(pa.locked, pb.locked);
            assert_eq!(a.effective_tower(pa), b.effective_tower(pb), "{}", pa.name);
            assert_eq!(pa.objects.len(), pb.objects.len(), "{}", pa.name);
            for (oa, ob) in pa.objects.iter().zip(&pb.objects) {
                assert_eq!(oa.name, ob.name);
                assert_eq!(oa.object_id, ob.object_id);
                assert_eq!(oa.instance_id, ob.instance_id);
                assert_eq!(oa.tools, ob.tools, "{}", oa.name);
                // Placed geometry: the same triangles (plates past the first go through the
                // grid offset and back, so to within a nanometre rather than bit-exact).
                let close = |a: &mica_core::mesh::TriangleMesh,
                             b: &mica_core::mesh::TriangleMesh,
                             what: &str| {
                    assert_eq!(a.faces.len(), b.faces.len(), "{}: {what}", oa.name);
                    for (fa, fb) in a.faces.iter().zip(&b.faces) {
                        for (ia, ib) in fa.iter().zip(fb) {
                            let (va, vb) = (a.vertices[*ia as usize], b.vertices[*ib as usize]);
                            for k in 0..3 {
                                assert!(
                                    (va[k] - vb[k]).abs() < 1e-4,
                                    "{}: {what} {va:?} {vb:?}",
                                    oa.name
                                );
                            }
                        }
                    }
                };
                close(&oa.placed_mesh(), &ob.placed_mesh(), "placed mesh");
                // A loose model comes back from a project as a one-part regions model.
                let regions = |o: &Object| {
                    o.placed_region()
                        .unwrap_or_else(|| mica_core::regions::RegionModel {
                            volumes: vec![mica_core::regions::Volume {
                                mesh: o.placed_mesh(),
                                kind: PartKind::Model,
                                config: Config::new(),
                                painting: FacetPainting::default(),
                            }],
                            height_ranges: vec![],
                            ..Default::default()
                        })
                };
                let (ra, rb) = (regions(oa), regions(ob));
                {
                    assert_eq!(ra.volumes.len(), rb.volumes.len(), "{}", oa.name);
                    for (va, vb) in ra.volumes.iter().zip(&rb.volumes) {
                        assert_eq!(va.kind, vb.kind);
                        assert_eq!(
                            va.painting.fingerprint(),
                            vb.painting.fingerprint(),
                            "{}: painting",
                            oa.name
                        );
                        close(&va.mesh, &vb.mesh, "volume");
                        // Typed values; an Orca import also keeps the source spelling as an
                        // unknown key.
                        let typed = |c: &mica_core::config::Config| {
                            c.values()
                                .map(|(k, v)| (k.to_string(), v.clone()))
                                .collect::<Vec<_>>()
                        };
                        assert_eq!(
                            typed(&va.config),
                            typed(&vb.config),
                            "{}: volume settings",
                            oa.name
                        );
                    }
                    assert_eq!(ra.height_ranges.len(), rb.height_ranges.len());
                }
                let known = |s: &std::collections::BTreeMap<String, String>| {
                    s.iter()
                        .filter(|(k, _)| mica_core::config::option_defs().contains_key(k.as_str()))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect::<Vec<_>>()
                };
                assert_eq!(oa.volumes.len(), ob.volumes.len());
                for (va, vb) in oa.volumes.iter().zip(&ob.volumes) {
                    assert_eq!(va.name, vb.name);
                    assert_eq!(va.kind, vb.kind);
                    assert_eq!(known(&va.overrides), known(&vb.overrides));
                }
                assert_eq!(
                    oa.height_ranges
                        .iter()
                        .map(|r| (r.min_z, r.max_z, known(&r.overrides)))
                        .collect::<Vec<_>>(),
                    ob.height_ranges
                        .iter()
                        .map(|r| (r.min_z, r.max_z, known(&r.overrides)))
                        .collect::<Vec<_>>()
                );
                assert_eq!(known(&oa.overrides), known(&ob.overrides));
            }
        }
    }

    fn gcode(app: &App) -> (Vec<u8>, Vec<(usize, f64)>) {
        let config = app.plate_config(app.plates.current).unwrap();
        let objects = app.plates.current().objects.clone();
        let finished = crate::worker::slice(
            objects,
            config,
            &std::sync::atomic::AtomicBool::new(false),
            &|_| {},
        )
        .unwrap();
        let bytes = std::fs::read(finished.preview.job.path.join("print.gcode")).unwrap();
        let tools = finished
            .summary
            .tools
            .iter()
            .map(|t| (t.tool, t.filament_mm))
            .collect();
        (bytes, tools)
    }

    /// The object tree's user groups survive a save and reopen in both flavours, live only in
    /// Mica's own metadata and do not touch the G-code.
    #[test]
    fn tree_groups_round_trip_in_mica_metadata_only() {
        use crate::commands::{Command, Shape};
        let ctx = egui::Context::default();
        for flavour in [Flavour::Orca, Flavour::Prusa] {
            let mut a = app(&ctx);
            open(&mut a, "cube20.stl");
            a.selected = Some(0);
            for _ in 0..3 {
                a.run_command(Command::AddModifier(Shape::Box), &ctx);
            }
            a.run_command(Command::AddHeightRange, &ctx);
            a.run_command(Command::AddHeightRange, &ctx);
            let before = gcode(&a);
            {
                let o = &mut a.plates.current_mut().objects[0];
                o.volumes[1].group = "Hull details".into();
                o.volumes[2].group = "Hull details".into();
                o.height_ranges[1].group = "Top".into();
            }
            assert_eq!(gcode(&a).0, before.0, "groups changed the G-code");
            let path = temp("groups.3mf");
            a.save_project_to(&path, flavour).unwrap();
            // Native model settings never carry them: the writer only puts a part's `matrix` /
            // `source_*` metadata there (`mica_core::threemf::write`), and object metadata
            // lives in `Metadata/mica.json` alone.
            let b = reopen(&ctx, &path, None);
            let o = &b.plates.current().objects[0];
            let groups: Vec<&str> = o.volumes.iter().map(|v| v.group.as_str()).collect();
            assert_eq!(groups, ["", "Hull details", "Hull details"], "{flavour:?}");
            let ranges: Vec<&str> = o.height_ranges.iter().map(|r| r.group.as_str()).collect();
            assert_eq!(ranges, ["", "Top"], "{flavour:?}");
        }
    }

    /// Texture paint slots (`mica_texture_paint_slots`, on the print and on an object) come back
    /// from an Orca project on the U1 and from a PrusaSlicer project (a former bug:
    /// the Orca import dropped every Mica object option as having no canonical equivalent).
    #[test]
    fn texture_paint_slots_round_trip() {
        let ctx = egui::Context::default();
        let slots = "1:texture=builtin:bark,amplitude=0.8;3:amplitude=0.2";
        for (flavour, printer) in [
            (Flavour::Orca, Some("Snapmaker U1 (0.4 nozzle)")),
            (Flavour::Prusa, None),
        ] {
            let mut a = app(&ctx);
            if let Some(p) = printer {
                if !a.profiles.all_printers().iter().any(|(_, _, n)| n == p) {
                    continue;
                }
                select_printer(&mut a, p);
            }
            open(&mut a, "cube20.stl");
            a.apply_edit(
                Some(crate::overlays::Target::Print),
                crate::panels::editor::Edit::Set {
                    key: "mica_texture_paint_slots".into(),
                    raw: slots.into(),
                },
            );
            a.plates.current_mut().objects[0]
                .overrides
                .insert("mica_texture_paint_slots".into(), "2:amplitude=1.5".into());
            let path = temp("slots.3mf");
            a.save_project_to(&path, flavour).unwrap();
            let b = reopen(&ctx, &path, printer);
            assert_eq!(
                b.plates.current().objects[0]
                    .overrides
                    .get("mica_texture_paint_slots")
                    .map(String::as_str),
                Some("2:amplitude=1.5"),
                "{flavour:?}: object slots"
            );
            let c = b.config.as_ref().unwrap();
            assert_eq!(
                c.raw("mica_texture_paint_slots").as_deref(),
                Some(slots),
                "{flavour:?}: print slots"
            );
        }
    }

    #[test]
    fn modified_flag_follows_edits_and_saves() {
        let ctx = egui::Context::default();
        let mut a = app(&ctx);
        assert!(!a.is_modified());
        assert_eq!(a.window_title(), "Untitled — Mica Slicer");
        open(&mut a, "cube20.stl");
        // Loose models are an import: unsaved.
        assert!(a.is_modified());
        assert_eq!(a.window_title(), "• Untitled — Mica Slicer");
        let path = temp("cube.3mf");
        a.save_project_to(&path, Flavour::Prusa).unwrap();
        assert!(!a.is_modified());
        assert_eq!(a.window_title(), "cube — Mica Slicer");
        assert_eq!(a.prefs.recent_files.first(), Some(&path));
        a.selected = Some(0);
        a.run_command(crate::commands::Command::Rotate90, &ctx);
        assert!(a.is_modified());
        a.undo();
        assert!(!a.is_modified());
        a.overlays.print.insert("perimeters".into(), "4".into());
        a.settings_changed();
        assert!(a.is_modified());
        // Opening a project over a modified scene asks first.
        a.load(vec![models().join("plates-orca.3mf")], &ctx);
        assert_eq!(
            a.dialogs.unsaved,
            Some(Pending::Open(vec![models().join("plates-orca.3mf")]))
        );
        assert_eq!(a.plates.current().objects.len(), 1);
        // An opened project is clean and named after its file, in its flavour.
        let mut b = reopen(&ctx, &path, None);
        assert!(!b.is_modified());
        assert_eq!(b.document_name(), "cube");
        assert_eq!(b.document.flavour, Some(Flavour::Prusa));
        assert_eq!(b.default_flavour(), Flavour::Prusa);
        b.load(vec![models().join("cube20.stl")], &ctx);
        assert!(b.dialogs.unsaved.is_none(), "models import without asking");
    }

    /// A colour picked for a tool is an unsaved filament change that keeps the slice (its
    /// export slices again first), and survives saving and reopening in both flavours.
    #[test]
    fn colour_pick_keeps_the_slice_and_round_trips_through_save_and_load() {
        use crate::panels::colour::Pick;
        let ctx = egui::Context::default();
        let mut a = app(&ctx);
        xl(&mut a);
        let printer = a.profiles.printer.clone();
        open(&mut a, "cube20.stl");
        let finished = crate::worker::slice(
            a.plates.current().objects.clone(),
            a.plate_config(a.plates.current).unwrap(),
            &std::sync::atomic::AtomicBool::new(false),
            &|_| {},
        )
        .unwrap();
        let inputs = std::sync::Arc::new(a.slice_inputs(a.plates.current).unwrap());
        a.store_slice(a.plates.current, Some(inputs), finished);
        let generation = a.config_generation;
        let teal = [0x1E, 0x9C, 0xA0];
        let orange = [0xF2, 0x79, 0x1F];
        a.set_filament_colour(
            1,
            Pick {
                colour: teal,
                alpha: None,
                commit: true,
            },
        );
        a.end_gesture();
        // A project colour, not a filament change.
        assert_eq!(a.overlays.colour(1), Some("#1E9CA0"));
        assert!(!a.overlays.filaments[1].contains_key("filament_colour"));
        assert_eq!(a.tool_colours()[1], teal);
        assert_eq!(
            a.config.as_ref().unwrap().string_at("filament_colour", 1),
            Some("#1E9CA0")
        );
        assert!(a.config_generation > generation);
        // The slice stays (the preview repaints with the new colour); exporting slices again.
        assert!(a.finished().is_some());
        assert_eq!(a.recent_colours(), vec![teal]);
        assert!(a.is_modified());
        // Another tool, through the settings editor's path.
        a.apply_edit(
            Some(crate::overlays::Target::Filament(3)),
            crate::panels::editor::Edit::Set {
                key: "filament_colour".into(),
                raw: crate::project::hex(orange),
            },
        );
        a.end_gesture();
        assert_eq!(a.tool_colours()[3], orange);
        assert!(a.finished().is_some(), "a colour edit keeps the slice");
        assert!(a.reslice_for_colours());
        assert!(a.finished().is_none() && a.reslice_requested);
        // Undo puts the old colour back (and redo the new one).
        a.undo();
        assert_eq!(a.overlays.colour(3), None);
        a.redo();
        assert_eq!(a.tool_colours()[3], orange);
        for flavour in [Flavour::Prusa, Flavour::Orca] {
            let path = temp(&format!("colour-{}.3mf", flavour.name()));
            a.save_project_to(&path, flavour).unwrap();
            let b = reopen(&ctx, &path, Some(&printer));
            assert_eq!(b.tool_colours()[1], teal, "{}", flavour.name());
            assert_eq!(b.tool_colours()[3], orange, "{}", flavour.name());
            assert_eq!(
                b.tool_colours()[0],
                a.tool_colours()[0],
                "{}",
                flavour.name()
            );
        }
    }

    /// A translucent filament colour (`#RRGGBBAA`, as Bambu Studio / Orca projects store
    /// it) survives import, the G-code's settings block, saving in both flavours and reopening
    /// byte for byte; opaque colours stay `#RRGGBB`. Changing only the RGB of a translucent
    /// colour keeps its alpha; an 8-digit hex pick sets a new one.
    #[test]
    fn translucent_filament_colour_round_trips_as_imported() {
        use crate::panels::colour::Pick;
        let ctx = egui::Context::default();
        let mut a = app(&ctx);
        xl(&mut a);
        let printer = a.profiles.printer.clone();
        // The synthetic Orca project with T2 translucent (upper case) and T3 lower case.
        let mut project = mica_core::threemf::load_3mf(models().join("plates-orca.3mf")).unwrap();
        let config = project.config.as_mut().unwrap();
        assert!(config.text.contains("\"#000000\"") && config.text.contains("\"#FFFFFF\""));
        config.text = config
            .text
            .replace("\"#000000\"", "\"#B5001180\"")
            .replace("\"#FFFFFF\"", "\"#ffffff40\"");
        let source = temp("translucent-source.3mf");
        mica_core::threemf::write(&source, &project, Flavour::Orca).unwrap();
        let result = crate::app::load_files(vec![source.clone()], &a.config, a.bed.center());
        a.finish_load(result);
        assert!(!a.plates.is_blank(), "{}", a.status);
        a.keep_my_settings();
        assert_eq!(a.overlays.colour(1), Some("#B5001180"));
        assert_eq!(a.overlays.colour(2), Some("#ffffff40"));
        assert_eq!(a.tool_colours_rgba()[1], [0xB5, 0x00, 0x11, 0x80]);
        assert_eq!(a.tool_colours()[1], [0xB5, 0x00, 0x11]);
        assert_eq!(a.tool_colours_rgba()[0], [0xCB, 0x8F, 0x29, 255]);
        let texts = a.tool_colour_texts();
        assert_eq!(&texts[..3], ["#CB8F29", "#B5001180", "#ffffff40"]);
        // The 3D view draws them translucent (0x80 / 255, and 0x40 / 255).
        let alphas = a.scene_alphas();
        assert!((alphas[1] - 128. / 255.).abs() < 1e-6 && alphas[0] == 1.);
        // The G-code's settings block keeps the text as imported.
        let (bytes, _) = gcode(&a);
        let text = String::from_utf8_lossy(&bytes);
        let line = text
            .lines()
            .find(|l| l.starts_with("; filament_colour ="))
            .expect("filament_colour in the G-code");
        assert!(line.contains("#CB8F29;#B5001180;#ffffff40"), "{line}");
        for flavour in [Flavour::Orca, Flavour::Prusa] {
            let path = temp(&format!("translucent-{}.3mf", flavour.name()));
            a.save_project_to(&path, flavour).unwrap();
            let saved = mica_core::threemf::load_3mf(&path).unwrap();
            let settings = &saved.config.as_ref().unwrap().text;
            assert!(
                settings.contains("#B5001180") && settings.contains("#ffffff40"),
                "{}: {settings}",
                flavour.name()
            );
            let b = reopen(&ctx, &path, Some(&printer));
            assert_eq!(
                b.tool_colours_rgba()[1],
                [0xB5, 0x00, 0x11, 0x80],
                "{}",
                flavour.name()
            );
            assert_eq!(b.tool_colour_texts()[2], "#ffffff40", "{}", flavour.name());
        }
        // A palette pick changes the colour, not the alpha.
        a.set_filament_colour(
            1,
            Pick {
                colour: [0x1E, 0x9C, 0xA0],
                alpha: None,
                commit: true,
            },
        );
        a.end_gesture();
        assert_eq!(a.overlays.colour(1), Some("#1E9CA080"));
        // The hex field's 6-digit colour makes it opaque; an 8-digit one sets the alpha.
        a.set_filament_colour(
            1,
            Pick {
                colour: [0x1E, 0x9C, 0xA0],
                alpha: Some(255),
                commit: true,
            },
        );
        assert_eq!(a.overlays.colour(1), Some("#1E9CA0"));
        a.set_filament_colour(
            1,
            Pick {
                colour: [0x1E, 0x9C, 0xA0],
                alpha: Some(0x33),
                commit: true,
            },
        );
        assert_eq!(a.overlays.colour(1), Some("#1E9CA033"));
        // The settings editor's swatch keeps the alpha of the text it edits.
        assert_eq!(
            crate::panels::colour::colour_text("#b5001180", [0xB5, 0, 0x11, 0x80]),
            "#b5001180"
        );
    }

    /// Orca flavour: the synthetic Orca project opened on the XL (its settings offered, kept;
    /// a Prusa printer, so the Orca settings are the translated canonical configuration), with
    /// a modifier, a height range, a mirrored instance and a paint stroke, saved and reopened
    /// gives the same scene and the same G-code.
    #[test]
    fn orca_roundtrip_keeps_scene_settings_and_gcode() {
        let ctx = egui::Context::default();
        let mut a = app(&ctx);
        xl(&mut a);
        let printer = a.profiles.printer.clone();
        open(&mut a, "plates-orca.3mf");
        a.keep_my_settings();
        assert_eq!(a.plates.len(), 2);
        a.select_plate(0);
        a.selected = Some(1);
        a.run_command(
            crate::commands::Command::AddModifier(crate::commands::Shape::Box),
            &ctx,
        );
        a.sub = None;
        a.run_command(crate::commands::Command::AddHeightRange, &ctx);
        a.sub = None;
        a.run_command(crate::commands::Command::Mirror(0), &ctx);
        a.run_command(crate::commands::Command::AddInstance, &ctx);
        a.selected = Some(0);
        a.run_command(
            crate::commands::Command::AddSupportBlocker(crate::commands::Shape::Cylinder),
            &ctx,
        );
        a.sub = None;
        {
            let o = &mut a.plates.current_mut().objects[0];
            o.apply(scene::Delta::Scale(glam::Vec3::splat(1.5)));
            o.apply(scene::Delta::Rotate {
                axis: glam::Vec3::Z,
                angle: 0.4,
            });
        }
        a.changed();
        // A support stroke on the first object.
        a.viewport.paint = Some(mica_core::painting::PaintKind::Supports);
        a.paint.brush = crate::paint::BrushKind::Sphere;
        a.paint.set_radius(5.);
        let o = a.plates.current().objects[0].clone();
        let (min, max) = o.bounds();
        let camera = glam::Vec3::new(max.x + 100., (min.y + max.y) / 2., 10.);
        let hit =
            scene::picking::pick_face(std::slice::from_ref(&o), camera, glam::Vec3::NEG_X).unwrap();
        let map = [(0usize, 0usize)];
        a.handle(
            crate::viewport::Action::Paint {
                index: 0,
                hit: crate::paint::Hit {
                    face: hit.face,
                    point: glam::Vec3::new(max.x, (min.y + max.y) / 2., 10.),
                    camera,
                },
                trail: vec![],
                erase: false,
            },
            &map,
            &ctx,
        );
        a.handle(crate::viewport::Action::Edited, &map, &ctx);
        a.viewport.paint = None;
        assert!(!a.plates.current().objects[0].painting.supports.is_empty());
        a.overlays.print.insert("perimeters".into(), "4".into());
        a.settings_changed();
        let before = gcode(&a);
        let path = temp("orca.3mf");
        a.save_project_to(&path, Flavour::Orca).unwrap();
        let project = mica_core::threemf::load_3mf(&path).unwrap();
        assert_eq!(project.plates.len(), 2);
        assert_eq!(project.objects.len(), 4);
        assert_eq!(project.objects[1].instances.len(), 2);
        assert!(
            project.objects[1]
                .parts
                .iter()
                .any(|p| p.kind == PartKind::Modifier)
        );
        assert_eq!(project.objects[1].height_ranges.len(), 1);
        let config = project.config.as_ref().unwrap();
        assert!(config.path.ends_with("project_settings.config"));
        // The XL has no Orca presets: the canonical configuration is translated.
        assert!(
            config.text.contains("\"wall_loops\":\"4\""),
            "{}",
            &config.text[..200]
        );
        let b = reopen(&ctx, &path, Some(&printer));
        same_scene(&a, &b);
        assert!(!b.is_modified());
        same_gcode(&before, &gcode(&b), "orca");
    }

    /// A cube cut with a plug, a dowel and a snap: the halves (a two-part lower object with
    /// negative volumes, an upper object with holes) and the dowel save as Orca and Prusa
    /// projects, reopen as the same scene and slice to the same G-code; the Orca file names
    /// its parts with Orca's volume types.
    #[test]
    fn cut_connectors_roundtrip_in_both_flavours() {
        use mica_core::cut_connectors::{Attributes, ConnectorType, Sizes};
        let ctx = egui::Context::default();
        let mut a = app(&ctx);
        open(&mut a, "cube20.stl");
        a.selected = Some(0);
        a.toggle_cut();
        let c = a.cut.as_mut().unwrap();
        c.place_on_cut = true;
        let sizes = Sizes {
            radius: 2.,
            height: 4.,
            ..Sizes::default()
        };
        for (x, kind) in [
            (-5., ConnectorType::Plug),
            (0., ConnectorType::Dowel),
            (5., ConnectorType::Snap),
        ] {
            c.connectors.push(crate::cut::Connector {
                at: glam::Vec2::new(x, 2.),
                attributes: Attributes {
                    kind,
                    ..Attributes::default()
                },
                sizes,
                auto: false,
            });
        }
        a.apply_cut();
        assert!(a.cut.is_none(), "{}", a.status);
        assert_eq!(a.plates.current().objects.len(), 3, "{}", a.status);
        let before = gcode(&a);
        assert!(!before.0.is_empty());
        for flavour in [Flavour::Orca, Flavour::Prusa] {
            let path = temp(&format!("cut-connectors-{flavour:?}.3mf"));
            a.save_project_to(&path, flavour).unwrap();
            let project = mica_core::threemf::load_3mf(&path).unwrap();
            assert_eq!(project.objects.len(), 3);
            let kinds = |i: usize| {
                project.objects[i]
                    .parts
                    .iter()
                    .map(|p| p.kind.clone())
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                kinds(0),
                vec![
                    PartKind::Model,
                    PartKind::NegativeVolume,
                    PartKind::NegativeVolume,
                    PartKind::NegativeVolume
                ]
            );
            assert_eq!(
                kinds(1),
                vec![
                    PartKind::Model,
                    PartKind::Model,
                    PartKind::Model,
                    PartKind::NegativeVolume
                ]
            );
            // (The reader takes Orca's `negative_part` / `normal_part` volume types and
            // PrusaSlicer's `NegativeVolume` / `ModelPart`, so the kinds above are the file's.)
            let b = reopen(&ctx, &path, None);
            same_scene(&a, &b);
            same_gcode(&before, &gcode(&b), &format!("{flavour:?}"));
        }
    }

    /// Prusa flavour on the XL: the painted cube with a colour stroke and a negative volume,
    /// saved and reopened, slices to the same G-code; the settings come back as the same
    /// effective configuration.
    #[test]
    fn prusa_roundtrip_on_the_xl_keeps_painting_and_gcode() {
        let ctx = egui::Context::default();
        let mut a = app(&ctx);
        xl(&mut a);
        let printer = a.profiles.printer.clone();
        open(&mut a, "painted-cube.3mf");
        a.selected = Some(0);
        a.run_command(
            crate::commands::Command::AddNegative(crate::commands::Shape::Cylinder),
            &ctx,
        );
        a.sub = None;
        // Colour the top with tool 3.
        a.viewport.paint = Some(mica_core::painting::PaintKind::Material);
        a.paint.brush = crate::paint::BrushKind::Sphere;
        a.paint.set_radius(6.);
        a.paint.tool = 2;
        let o = a.plates.current().objects[0].clone();
        let (min, max) = o.bounds();
        let camera = glam::Vec3::new((min.x + max.x) / 2., (min.y + max.y) / 2., max.z + 100.);
        let hit =
            scene::picking::pick_face(std::slice::from_ref(&o), camera, glam::Vec3::NEG_Z).unwrap();
        let map = [(0usize, 0usize)];
        a.handle(
            crate::viewport::Action::Paint {
                index: 0,
                hit: crate::paint::Hit {
                    face: hit.face,
                    point: glam::Vec3::new((min.x + max.x) / 2., (min.y + max.y) / 2., max.z),
                    camera,
                },
                trail: vec![],
                erase: false,
            },
            &map,
            &ctx,
        );
        a.handle(crate::viewport::Action::Edited, &map, &ctx);
        a.viewport.paint = None;
        a.overlays.filaments[1].insert("temperature".into(), "231".into());
        a.settings_changed();
        let before = gcode(&a);
        assert!(before.1.len() >= 2, "{:?}", before.1);
        let path = temp("prusa.3mf");
        a.save_project_to(&path, Flavour::Prusa).unwrap();
        let project = mica_core::threemf::load_3mf(&path).unwrap();
        let text = &project.config.as_ref().unwrap().text;
        assert!(text.contains("; temperature = "));
        assert!(
            text.contains("; printer_settings_id = Original Prusa XL"),
            "{text}"
        );
        let b = reopen(&ctx, &path, Some(&printer));
        // The project names the selected XL presets: used without asking, with its saved
        // value on tool 2 (the project's own, not an edit).
        assert!(matches!(
            b.project.as_ref().map(|p| &p.state),
            Some(crate::project::State::Applied { .. })
        ));
        assert_eq!(b.profiles.printer, a.profiles.printer);
        assert_eq!(b.tool_count(), a.tool_count());
        assert!(b.overlays.filaments[1].is_empty());
        assert_eq!(
            b.config.as_ref().unwrap().float_at("temperature", 1),
            Some(231.)
        );
        assert!(
            b.project_differences
                .iter()
                .any(|d| d.key == "temperature" && d.tool == Some(1))
        );
        let ca = a.config.as_ref().unwrap();
        let cb = b.config.as_ref().unwrap();
        for (k, _) in ca.values() {
            if k.ends_with("_settings_id") || k == "filament_colour" {
                continue;
            }
            assert_eq!(ca.raw(k), cb.raw(k), "{k}");
        }
        same_scene(&a, &b);
        same_gcode(&before, &gcode(&b), "prusa");
    }

    /// Snapmaker U1 (needs the pinned Orca profiles; a reported skip without them): a project made
    /// for the U1 is used as is and saved with its settings text verbatim; an edit is applied
    /// on top and listed in `different_settings_to_system`, where a reopening Mica finds it; a
    /// loose model saved as an Orca project gets the composed presets with one entry per slot.
    #[test]
    fn u1_projects_keep_native_settings_and_list_edits() {
        if !mica_testkit::require_orca_profiles() {
            return;
        }
        let ctx = egui::Context::default();
        let mut a = app(&ctx);
        let (vi, _, u1) = a
            .profiles
            .all_printers()
            .into_iter()
            .find(|(_, _, n)| n == "Snapmaker U1 (0.4 nozzle)")
            .expect("the Snapmaker U1 among the loaded printers");
        a.profiles.select_printer(vi, &u1);
        a.settings_changed();
        open(&mut a, "plates-orca.3mf");
        assert!(matches!(
            a.project.as_ref().map(|p| &p.state),
            Some(crate::project::State::Applied { .. })
        ));
        assert_eq!(a.default_flavour(), Flavour::Orca);
        assert!(!a.is_modified());
        let original = a.document.original.clone().unwrap();
        let path = temp("u1.3mf");
        a.save_project_to(&path, Flavour::Orca).unwrap();
        let saved = mica_core::threemf::load_3mf(&path).unwrap();
        assert_eq!(
            saved.config.as_ref().unwrap().text,
            original.text,
            "verbatim settings"
        );
        assert_eq!(saved.plates.len(), 2);
        assert_eq!(saved.wipe_towers.get(&0).map(|t| t[0]), Some(165.));
        // An edit on top: applied as a full per-slot vector and listed for Orca and Mica.
        a.overlays.print.insert("perimeters".into(), "5".into());
        a.overlays.filaments[1].insert("temperature".into(), "231".into());
        a.settings_changed();
        a.save_project_to(&path, Flavour::Orca).unwrap();
        let saved = mica_core::threemf::load_3mf(&path).unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&saved.config.as_ref().unwrap().text).unwrap();
        assert_eq!(json["wall_loops"], "5");
        let temps: Vec<&str> = json["nozzle_temperature"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(temps.len(), 4);
        assert_eq!(temps[1], "231");
        let diff: Vec<&str> = json["different_settings_to_system"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(diff.len(), 6);
        assert!(diff[0].split(';').any(|k| k == "wall_loops"), "{diff:?}");
        assert!(
            diff[0].split(';').any(|k| k == "sparse_infill_density"),
            "{diff:?}"
        );
        assert!(
            diff[2].split(';').any(|k| k == "nozzle_temperature"),
            "{diff:?}"
        );
        // Reopened: the saved values are the project's own (sliced with, not edits).
        let b = reopen(&ctx, &path, Some(&u1));
        let cb = b.config.as_ref().unwrap();
        assert_eq!(cb.int("perimeters"), Some(5));
        assert_eq!(cb.float_at("temperature", 1), Some(231.));
        assert_eq!(cb.float("fill_density"), Some(8.));
        assert!(b.overlays.print.is_empty() && b.overlays.filaments.iter().all(|f| f.is_empty()));
        same_scene(&a, &b);
        // A loose model: Orca's own composition of the selected presets.
        let mut c = app(&ctx);
        c.profiles.select_printer(vi, &u1);
        c.settings_changed();
        open(&mut c, "cube20.stl");
        c.overlays.filaments[2].insert("temperature".into(), "199".into());
        c.settings_changed();
        let path = temp("u1-cube.3mf");
        c.save_project_to(&path, Flavour::Orca).unwrap();
        let saved = mica_core::threemf::load_3mf(&path).unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&saved.config.as_ref().unwrap().text).unwrap();
        assert_eq!(json["printer_settings_id"], u1.as_str());
        assert_eq!(json["print_settings_id"], c.profiles.process.as_str());
        let tools = c.tool_count();
        assert_eq!(tools, 4);
        for key in [
            "filament_settings_id",
            "filament_colour",
            "nozzle_temperature",
            "filament_type",
            "filament_is_support",
            "nozzle_diameter",
            "retraction_length",
        ] {
            let n = json[key].as_array().map(Vec::len);
            assert_eq!(n, Some(tools), "{key}: {}", json[key]);
        }
        assert_eq!(json["nozzle_temperature"][2], "199");
        assert_eq!(json["curr_bed_type"], c.profiles.bed.as_str());
        let diff = json["different_settings_to_system"].as_array().unwrap();
        assert_eq!(diff[3], "nozzle_temperature");
        let d = reopen(&ctx, &path, Some(&u1));
        assert_eq!(
            d.config.as_ref().unwrap().float_at("temperature", 2),
            Some(199.)
        );
        assert!(d.overlays.filaments[2].is_empty());
        same_scene(&c, &d);
        // A loose model slices through the engine's plain path, its project copy through the
        // regions path (as every 3MF object does); the paths differ by a few hundredths of a
        // percent of filament on the cube (first-layer perimeter corners), not by geometry.
        let (before, after) = (gcode(&c), gcode(&d));
        assert_eq!(before.1.len(), after.1.len());
        for ((ta, fa), (tb, fb)) in before.1.iter().zip(&after.1) {
            assert_eq!(ta, tb);
            assert!((fa - fb).abs() / fa < 1e-3, "{fa} vs {fb}");
        }
    }

    /// Byte-identical G-code; on a mismatch both files land in the temp dir for diffing.
    fn same_gcode(a: &(Vec<u8>, Vec<(usize, f64)>), b: &(Vec<u8>, Vec<(usize, f64)>), what: &str) {
        if a.0 != b.0 {
            let dir = std::env::temp_dir();
            std::fs::write(dir.join(format!("mica-{what}-before.gcode")), &a.0).unwrap();
            std::fs::write(dir.join(format!("mica-{what}-after.gcode")), &b.0).unwrap();
        }
        assert_eq!(a.1, b.1, "{what}: per-tool filament");
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
        assert_eq!(program(&a.0), program(&b.0), "{what}: G-code commands");
    }

    /// A calibration plate survives Save and Open in both flavours: which plate it is, its
    /// test, tool, parameters and typed reading come back from Mica's own metadata entry,
    /// (the native plate metadata does not carry it: see
    /// `mica_core/tests/threemf_write.rs::mica_plate_metadata_stays_in_the_sidecar`).
    #[test]
    fn calibration_plates_round_trip_through_the_project() {
        use crate::calibration::{PLATE_METADATA_KEY, Test};
        let ctx = egui::Context::default();
        let mut a = app(&ctx);
        xl(&mut a);
        let printer = a.profiles.printer.clone();
        open(&mut a, "painted-cube.3mf");
        a.open_calibration(Test::PaTower);
        let config = a.config.clone().unwrap();
        let card = a.dialogs.calibration.as_mut().unwrap();
        card.set_tool(2, &config);
        card.params.start = 0.01;
        card.params.speed = Some(120.);
        let plate = a.create_calibration_plate(&ctx).unwrap();
        a.cancel_slice();
        a.plates.plates[plate].calibration.as_mut().unwrap().input = "12.5".into();
        let job = a.plates.plate(plate).calibration.clone().unwrap();
        assert_eq!((job.tool, job.params.tool), (2, 2));
        let outcome = a.calibration_outcome().unwrap().unwrap();
        for flavour in [Flavour::Orca, Flavour::Prusa] {
            let path = temp(&format!("calibration-{}.3mf", flavour.name()));
            a.save_project_to(&path, flavour).unwrap();
            let b = reopen(&ctx, &path, Some(&printer));
            assert_eq!(b.plates.len(), a.plates.len());
            for (pa, pb) in a.plates.plates.iter().zip(&b.plates.plates) {
                assert_eq!(pa.calibration, pb.calibration, "{flavour:?} {}", pa.name);
                assert_eq!(pa.locked, pb.locked);
                assert!(!pb.metadata.contains_key(PLATE_METADATA_KEY));
            }
            assert!(b.plates.plate(0).calibration.is_none());
            let mut b = b;
            b.plates.current = plate;
            assert_eq!(b.calibration_outcome().unwrap().unwrap(), outcome);
            // A reopened project is unmodified, and saving it again keeps the job.
            assert!(!b.is_modified(), "{flavour:?}");
        }
    }

    /// Plate names and plate order (after a rename and a move) come back from a project in
    /// both flavours; an unnamed plate is stored without a name (Orca's empty `plater_name`)
    /// and is numbered by its position again.
    #[test]
    fn plate_names_and_order_round_trip_in_both_flavours() {
        let ctx = egui::Context::default();
        let mut a = app(&ctx);
        open(&mut a, "plates-orca.3mf");
        assert_eq!(a.plates.len(), 2);
        a.plates.add();
        a.rename_plate(0, "Gift for Tom");
        // Plate 2 ("Lid", locked, one object) first; the unnamed third plate stays last.
        a.move_plate(1, 0);
        let names =
            |a: &App| -> Vec<String> { a.plates.plates.iter().map(|p| p.name.clone()).collect() };
        let objects = |a: &App| -> Vec<Vec<String>> {
            a.plates
                .plates
                .iter()
                .map(|p| p.objects.iter().map(|o| o.name.clone()).collect())
                .collect()
        };
        assert_eq!(names(&a), ["Lid", "Gift for Tom", "Plate 3"]);
        assert!(a.plates.plate(0).locked && !a.plates.plate(1).locked);
        for flavour in [Flavour::Orca, Flavour::Prusa] {
            let path = temp(&format!("plate-names-{}.3mf", flavour.name()));
            a.save_project_to(&path, flavour).unwrap();
            let stored: Vec<String> = mica_core::threemf::load_3mf(&path)
                .unwrap()
                .plates
                .into_iter()
                .map(|p| p.name)
                .collect();
            assert_eq!(stored, ["Lid", "Gift for Tom", ""], "{flavour:?}");
            let b = reopen(&ctx, &path, None);
            assert_eq!(names(&b), names(&a), "{flavour:?}");
            assert_eq!(objects(&b), objects(&a), "{flavour:?}");
            assert!(b.plates.plate(0).locked, "{flavour:?}");
            assert!(!b.is_modified(), "{flavour:?}");
        }
    }

    #[test]
    fn tool_keys_and_overlay_arithmetic() {
        let mut s = Settings::new();
        tool_settings(&ToolAssignments([Some(1); 5]), None, &mut s);
        assert_eq!(s["extruder"], "2");
        assert_eq!(s["support_material_extruder"], "2");
        let mut s = Settings::new();
        let defaults = ToolAssignments([Some(0), Some(0), Some(0), None, None]);
        tool_settings(
            &ToolAssignments([Some(0), Some(2), Some(0), None, Some(3)]),
            Some(&defaults),
            &mut s,
        );
        assert_eq!(s["perimeter_extruder"], "1");
        assert_eq!(s["infill_extruder"], "3");
        assert!(!s.contains_key("extruder"));
        assert!(!s.contains_key("support_material_extruder"));
        assert_eq!(s["support_material_interface_extruder"], "4");
        let mut all = Overlays::new(2);
        all.print.insert("perimeters".into(), "4".into());
        all.filaments[1].insert("temperature".into(), "231".into());
        let mut applied = Overlays::new(2);
        applied.print.insert("perimeters".into(), "4".into());
        assert!(contains(&all, &applied));
        assert!(!contains(&applied, &all));
        let edits = minus(&all, &applied);
        assert!(edits.print.is_empty());
        assert_eq!(edits.filaments[1]["temperature"], "231");
        assert!(!is_empty(&edits));
        assert!(is_project(&[PathBuf::from("a.3MF")]));
        assert!(!is_project(&[PathBuf::from("a.stl")]));
    }
}
