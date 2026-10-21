// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Geometry and structure import/export for core, PrusaSlicer and Orca/Bambu 3MF projects.
//!
//! All coordinates are millimetres. Part meshes are in object coordinates (component
//! transforms are baked in); instance transforms place them on the build bed. Slicer
//! settings are retained verbatim for the print-job adapter; height ranges are imported separately.
//! Geometry import itself does not repair or Boolean meshes. See `docs/3mf-import.md` for the supported subset and oracle results.

mod assemble;
mod custom_gcode;
pub(crate) mod json;
pub mod layer_heights;
mod marker;
mod metadata;
mod paint;
mod parse;
mod ranges;
mod source;
mod tower;
mod write;
mod write_config;
mod xml;

pub use custom_gcode::Templates as CustomGCodeTemplates;
pub use source::SourceMesh;

pub use write::{Flavour, write};
pub use write_config::{source_flavour, translate};

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{Read, Seek};
use std::path::Path;

use crate::mesh::TriangleMesh;

pub type Settings = BTreeMap<String, String>;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ResourceId {
    pub path: String,
    pub id: u32,
}

/// Row-major affine matrix, operating on column vectors, with translation in column 3.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform(pub [[f64; 4]; 3]);

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Transform {
    pub const IDENTITY: Self = Self([
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
    ]);

    pub fn apply(self, p: [f32; 3]) -> [f32; 3] {
        self.0.map(|r| {
            (r[0] * f64::from(p[0]) + r[1] * f64::from(p[1]) + r[2] * f64::from(p[2]) + r[3]) as f32
        })
    }

    /// `self * child`: applies `child` first, then `self`.
    pub fn compose(self, child: Self) -> Self {
        let mut result = Self::IDENTITY;
        for (i, row) in result.0.iter_mut().enumerate() {
            for (j, value) in row.iter_mut().enumerate() {
                *value = (0..3).map(|k| self.0[i][k] * child.0[k][j]).sum::<f64>()
                    + if j == 3 { self.0[i][3] } else { 0.0 };
            }
        }
        result
    }

    /// Inverse of a finite non-singular affine transform.
    pub fn inverse(self) -> Option<Self> {
        let d = self.determinant();
        if !d.is_finite() || d == 0. {
            return None;
        }
        let mut out = Self::IDENTITY;
        for i in 0..3 {
            for j in 0..3 {
                let (a, b) = ((j + 1) % 3, (j + 2) % 3);
                let (c, e) = ((i + 1) % 3, (i + 2) % 3);
                out.0[i][j] = (self.0[a][c] * self.0[b][e] - self.0[a][e] * self.0[b][c]) / d;
            }
            out.0[i][3] = -(0..3).map(|k| out.0[i][k] * self.0[k][3]).sum::<f64>();
        }
        out.0.iter().flatten().all(|v| v.is_finite()).then_some(out)
    }

    pub fn determinant(self) -> f64 {
        let [a, b, c] = self.0;
        a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
            + a[2] * (b[0] * c[1] - b[1] * c[0])
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PartKind {
    Model,
    NegativeVolume,
    Modifier,
    SupportEnforcer,
    SupportBlocker,
    /// Retained but excluded from slicing, so an unknown type cannot silently add geometry.
    Unknown(String),
}

#[derive(Clone, Debug)]
pub struct Part {
    pub name: String,
    pub kind: PartKind,
    pub source: ResourceId,
    /// Mesh in object coordinates, in mm; `transform` is already baked into these vertices.
    pub mesh: TriangleMesh,
    /// Original double-precision vertices and their component placement, retained for saving.
    /// The writer uses this only while it still matches `mesh`.
    pub source_mesh: Option<std::sync::Arc<SourceMesh>>,
    pub painting: crate::painting::FacetPainting,
    /// Accumulated component transform, retained for inspection; do not apply it again.
    pub transform: Transform,
    pub settings: Settings,
    /// Structural/source metadata, including the slicer's unapplied source `matrix`.
    pub metadata: Settings,
}

#[derive(Clone, Debug)]
pub struct Instance {
    pub transform: Transform,
    pub printable: bool,
    /// Orca plate ID, as stored (usually one-based). A resource can have instances on different plates.
    pub plate: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct HeightRange {
    pub min_z: f64,
    pub max_z: f64,
    pub settings: Settings,
}

#[derive(Clone, Debug)]
pub struct Object {
    pub height_ranges: Vec<HeightRange>,
    /// Variable layer height profile, `(z, height)` pairs from the object's bottom
    /// (`Metadata/Slic3r_PE_layer_heights_profile.txt` / Orca's `layer_heights_profile.txt`).
    pub layer_height_profile: Vec<f64>,
    /// Orca's hand-placed brim ears, `[x, y, z, radius]` (`Metadata/brim_ear_points.txt`), as
    /// the file stores them: relative to [`Object::brim_frame_origin`], because OrcaSlicer
    /// recentres a loaded object on its bounding box and reads the points in that frame.
    pub brim_points: Vec<[f32; 4]>,
    pub name: String,
    pub source: ResourceId,
    pub parts: Vec<Part>,
    /// Build order within this resource; Orca's `instance_id` is an index into this list.
    pub instances: Vec<Instance>,
    pub settings: Settings,
    pub metadata: Settings,
}

impl Object {
    /// Merge model parts at every printable build instance, preserving XYZ placement.
    pub fn placed_model_mesh(&self) -> Result<TriangleMesh, ThreeMfError> {
        let mut mesh = TriangleMesh::default();
        self.append_placed_model_mesh(&mut mesh)?;
        Ok(mesh)
    }

    /// The origin of the brim ear frame in object coordinates: the centre of the bounding box
    /// of all parts (OrcaSlicer's `ModelObject::center_around_origin`, which a loaded project
    /// goes through without moving its `brim_points`). Orca's own files store centred
    /// meshes, so this is about zero for them.
    pub fn brim_frame_origin(&self) -> [f64; 3] {
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for v in self.parts.iter().flat_map(|p| &p.mesh.vertices) {
            for a in 0..3 {
                lo[a] = lo[a].min(f64::from(v[a]));
                hi[a] = hi[a].max(f64::from(v[a]));
            }
        }
        if lo[0] > hi[0] {
            return [0.; 3];
        }
        std::array::from_fn(|a| 0.5 * (lo[a] + hi[a]))
    }

    fn append_placed_model_mesh(&self, mesh: &mut TriangleMesh) -> Result<(), ThreeMfError> {
        for instance in self.instances.iter().filter(|i| i.printable) {
            for part in self.parts.iter().filter(|p| p.kind == PartKind::Model) {
                append_mesh(mesh, &part.mesh, instance.transform)?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct PlateInstance {
    /// Index into `Project::objects`.
    pub object: usize,
    /// Index into `Object::instances`.
    pub instance: usize,
}

#[derive(Clone, Debug)]
pub struct Plate {
    pub id: u32,
    pub name: String,
    pub instances: Vec<PlateInstance>,
    pub metadata: Settings,
}

#[derive(Clone, Debug)]
pub struct RawConfig {
    pub path: String,
    /// Original INI or JSON text. No parsing or application of printer/process/filament settings.
    pub text: String,
}

#[derive(Clone, Debug, Default)]
pub struct Project {
    /// Verbatim ColorMix metadata (including unknown fields and whitespace), retained in either 3MF dialect.
    pub full_spectrum_json: Option<String>,
    /// Only build-referenced objects, in first build occurrence order. Resources are parts, not extra objects.
    pub objects: Vec<Object>,
    /// Empty for files without explicit plate metadata.
    pub plates: Vec<Plate>,
    pub config: Option<RawConfig>,
    /// Tower placement, keyed by zero-based bed index: X, Y, rotation degrees.
    /// Includes Prusa model metadata and Orca project-setting vectors.
    pub wipe_towers: BTreeMap<u32, [f64; 3]>,
    /// Per-layer custom G-code (colour changes, pauses, …), keyed by zero-based plate (bed)
    /// index, from PrusaSlicer's `custom_gcode_per_print_z` or Orca's `custom_gcode_per_layer`.
    pub custom_gcodes: BTreeMap<u32, crate::custom_gcode::Info>,
    /// Printer templates for the legacy `gcode` attribute when writing `custom_gcodes`.
    pub custom_gcode_templates: custom_gcode::Templates,
    /// Features detected but not imported (thumbnails, etc.).
    pub ignored_features: BTreeSet<String>,
    /// Mica's own options (`mica_*`, e.g. `mica_slicing_rules`) of an Orca project. OrcaSlicer
    /// files carry only Orca options, so these live in Mica's sidecar (`Metadata/mica.json`);
    /// Prusa projects keep them in `Slic3r_PE.config`, which PrusaSlicer ignores unknown keys in.
    pub mica_settings: Settings,
}

impl Project {
    pub fn placed_model_mesh(&self) -> Result<TriangleMesh, ThreeMfError> {
        let mut mesh = TriangleMesh::default();
        for object in &self.objects {
            object.append_placed_model_mesh(&mut mesh)?;
        }
        Ok(mesh)
    }
}

#[derive(Debug)]
pub enum ThreeMfError {
    Io(std::io::Error),
    Zip(zip::result::ZipError),
    Invalid(String),
}

impl fmt::Display for ThreeMfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Zip(e) => write!(f, "3MF archive: {e}"),
            Self::Invalid(e) => write!(f, "invalid 3MF: {e}"),
        }
    }
}
impl std::error::Error for ThreeMfError {}
impl From<std::io::Error> for ThreeMfError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<zip::result::ZipError> for ThreeMfError {
    fn from(e: zip::result::ZipError) -> Self {
        Self::Zip(e)
    }
}
pub(crate) fn invalid(message: impl Into<String>) -> ThreeMfError {
    ThreeMfError::Invalid(message.into())
}

pub fn load_3mf(path: impl AsRef<Path>) -> Result<Project, ThreeMfError> {
    read_3mf(std::fs::File::open(path)?)
}

/// Read a seekable 3MF stream. XML is decompressed and parsed incrementally; no extraction to disk.
pub fn read_3mf(reader: impl Read + Seek) -> Result<Project, ThreeMfError> {
    parse::load(reader)
}

fn append_mesh(
    out: &mut TriangleMesh,
    mesh: &TriangleMesh,
    transform: Transform,
) -> Result<(), ThreeMfError> {
    let base = u32::try_from(out.vertices.len()).map_err(|_| invalid("too many vertices"))?;
    let count = u32::try_from(mesh.vertices.len()).map_err(|_| invalid("too many vertices"))?;
    base.checked_add(count)
        .ok_or_else(|| invalid("too many vertices"))?;
    for &vertex in &mesh.vertices {
        let vertex = transform.apply(vertex);
        if vertex.iter().any(|v| !v.is_finite()) {
            return Err(invalid("transformed coordinate overflow"));
        }
        out.vertices.push(vertex);
    }
    let mirrored = transform.determinant() < 0.0;
    for face in &mesh.faces {
        let mut face = face.map(|i| i + base);
        if mirrored {
            face.swap(1, 2);
        }
        out.faces.push(face);
    }
    Ok(())
}

/// Where Mica keeps custom texture images in a project (lower case).
pub(crate) const TEXTURES: &str = "metadata/mica/textures/";

/// Makes the project's custom texture images (stored by content hash) available for slicing
/// and for saving the project again.
pub(crate) fn register_textures<R: Read + Seek>(
    zip: &mut zip::ZipArchive<R>,
) -> Result<(), ThreeMfError> {
    let names: Vec<String> = zip
        .file_names()
        .filter(|n| n.to_ascii_lowercase().starts_with(TEXTURES) && n.ends_with(".png"))
        .map(str::to_owned)
        .collect();
    for name in names {
        let mut png = Vec::new();
        zip.by_name(&name)?
            .take(64 * 1024 * 1024 + 1)
            .read_to_end(&mut png)?;
        if png.len() > 64 * 1024 * 1024 {
            return Err(invalid("texture image exceeds 64 MiB"));
        }
        crate::texture::register_stored(png).map_err(|e| invalid(format!("{name}: {e}")))?;
    }
    Ok(())
}

/// Read only embedded preset settings, without loading or rounding project geometry.
/// Mica's own options an Orca project keeps in Mica's sidecar (empty without a current one).
pub fn load_mica_settings(path: impl AsRef<Path>) -> Result<Settings, ThreeMfError> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    register_textures(&mut zip)?;
    marker::mica_settings(&mut zip)
}

pub fn load_config(path: impl AsRef<Path>) -> Result<Option<RawConfig>, ThreeMfError> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    register_textures(&mut zip)?;
    if let Some(config) = marker::config(&mut zip)? {
        return Ok(config);
    }
    for path in [
        "Metadata/project_settings.config",
        "Metadata/Slic3r_PE.config",
    ] {
        match zip.by_name(path) {
            Ok(mut file) => {
                let mut text = String::new();
                file.read_to_string(&mut text)?;
                return Ok(Some(RawConfig {
                    path: path.into(),
                    text,
                }));
            }
            Err(zip::result::ZipError::FileNotFound) => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(None)
}
