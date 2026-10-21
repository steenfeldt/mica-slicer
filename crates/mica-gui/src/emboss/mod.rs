// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Gizmos/GLGizmoEmboss.cpp :: GLGizmoEmboss::create_volume
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Jobs/EmbossJob.cpp :: create_volume
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Jobs/EmbossJob.cpp :: try_create_mesh
// Source: orca@8500fcdcca src/slic3r/GUI/Gizmos/GLGizmoEmboss.cpp :: GLGizmoEmboss::on_shortcut_key
// Changes: Mesh rebuilds run from the stored config, at once for flat text and on one background thread for text wrapped onto a surface (no job queue); the four kinds (raised part, engraved negative volume, modifier, separate object) map onto Mica volumes / objects; the text frame is kept relative to the volume's mesh so gizmo moves and 3MF re-imports keep it; an imported part's `<slic3rpe:shape transform>` restores the frame.
// Upstream attribution (GLGizmoEmboss.cpp, EmbossJob.cpp):
// Copyright (c) Prusa Research 2021 - 2023 Filip Sykala @Jony01, Oleksandra Iushchenko @YuSanka, Lukáš Matěna @lukasmatena, Vojtěch Bubník @bubnikv, Enrico Turri @enricoturri1966
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher

//! The Text / SVG emboss tool (PrusaSlicer's GLGizmoEmboss and GLGizmoSVG, Orca's Text tool):
//! text in any installed font or an SVG file's fills, raised on a surface, engraved into it,
//! as a modifier or as an object of its own. The source stays with the volume, so the tool
//! re-opens on it and the project keeps it.

#[cfg(test)]
mod editor_tests;
pub mod fonts;
mod panel;
pub mod svg;
pub mod text;
#[cfg(test)]
mod ui_tests;

use glam::{Mat4, Vec3};
use mica_core::emboss::{EmbossConfig, EmbossRange, EmbossSource, SurfaceProbe};
use mica_core::geometry::ExPolygon;
use mica_core::mesh::TriangleMesh;
use mica_core::threemf::{PartKind, Transform};
use std::sync::Arc;

/// An embossed volume's (or object's) source and where its text frame sits in the mesh's own
/// coordinates (the volume's mesh, or the object's mesh for a separate text object).
#[derive(Clone, Debug, PartialEq)]
pub struct Emboss {
    pub config: EmbossConfig,
    pub frame: Mat4,
}

/// What the embossed shape is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Added on the surface (a model part).
    Raised,
    /// Cut into the surface (a negative volume).
    Engraved,
    /// A modifier with its own settings.
    Modifier,
    /// An object of its own, lying on the bed.
    Object,
}
impl Kind {
    pub fn part_kind(self) -> PartKind {
        match self {
            Kind::Raised | Kind::Object => PartKind::Model,
            Kind::Engraved => PartKind::NegativeVolume,
            Kind::Modifier => PartKind::Modifier,
        }
    }
    pub fn of(kind: &PartKind) -> Self {
        match kind {
            PartKind::NegativeVolume => Kind::Engraved,
            PartKind::Modifier => Kind::Modifier,
            _ => Kind::Raised,
        }
    }
    /// PrusaSlicer's `is_outside`: only a model part stands out of the surface.
    pub fn outside(self) -> bool {
        matches!(self, Kind::Raised | Kind::Object)
    }
    pub fn label(self) -> &'static str {
        match self {
            Kind::Raised => "Raised",
            Kind::Engraved => "Engraved",
            Kind::Modifier => "Modifier",
            Kind::Object => "Object",
        }
    }
}

/// The 2D outline of a config in the text frame (scaled mm) and warnings worth showing.
pub fn outline(config: &EmbossConfig) -> Result<(Vec<ExPolygon>, Vec<String>), String> {
    match &config.source {
        EmbossSource::Text(t) => {
            if t.text.trim().is_empty() {
                return Err("Type some text".into());
            }
            let lib = fonts::library();
            let face = lib.find(&t.family, &t.face, &t.file_name, t.bold, t.italic);
            let data =
                fonts::load(face).ok_or_else(|| format!("Cannot read the font {}", face.family))?;
            let out = text::outline(t, face, &data)?;
            let mut warnings = Vec::new();
            if !out.missing.is_empty() {
                warnings.push(format!(
                    "{} has no {}",
                    face.family,
                    out.missing
                        .iter()
                        .map(|c| format!("“{c}”"))
                        .collect::<Vec<_>>()
                        .join(" ")
                ));
            }
            if out.shape.is_empty() {
                return Err(format!("{} draws nothing for this text", face.family));
            }
            Ok((out.shape, warnings))
        }
        EmbossSource::Svg(s) => {
            let out = svg::outline(s)?;
            Ok((out.shape, out.warnings))
        }
    }
}

/// The units per em of the font a text config resolves to (for its mm ↔ font-unit gaps).
pub fn units_per_em(config: &EmbossConfig) -> Option<u16> {
    let t = config.text()?;
    let lib = fonts::library();
    let face = lib.find(&t.family, &t.face, &t.file_name, t.bold, t.italic);
    let data = fonts::load(face)?;
    Some(text::units_per_em(face, &data))
}

/// The mesh of `config` in its text frame. `surface` (meshes with transforms into the text
/// frame) wraps it onto curved surfaces when the config asks for it.
pub fn build(
    config: &EmbossConfig,
    kind: Kind,
    surface: &[(&TriangleMesh, Transform)],
) -> Result<(TriangleMesh, Vec<String>), String> {
    let (shape, mut warnings) = outline(config)?;
    let range = EmbossRange::new(kind.outside(), config.depth);
    let (lo, hi) = bounds(&shape);
    let probe = (config.use_surface && !surface.is_empty())
        .then(|| {
            let cell = ((hi[0] - lo[0]).max(hi[1] - lo[1]) / 64.).max(0.5);
            SurfaceProbe::new(surface, (lo, hi), cell)
        })
        .filter(|p| !p.is_empty());
    if config.use_surface && probe.is_none() && !surface.is_empty() {
        warnings.push("No surface under the text: it stays flat".into());
    }
    // Wrapped, the shape keeps only what lies over the surface (PrusaSlicer's surface cut).
    let shape = match &probe {
        Some(p) => {
            let area = |s: &[ExPolygon]| s.iter().map(ExPolygon::area_mm2).sum::<f64>();
            let cut = mica_core::geometry::clip::intersection_ex(&shape, &p.coverage());
            if area(&cut) < area(&shape) - 0.01 {
                warnings
                    .push("Part of the shape runs past the surface's edge and is left out".into());
            }
            if cut.is_empty() {
                return Err("The shape does not lie over the surface".into());
            }
            cut
        }
        None => shape,
    };
    let max_edge = ((hi[0] - lo[0]).max(hi[1] - lo[1]) / 150.).max(0.4);
    let mesh = mica_core::emboss::emboss_mesh(&shape, range, probe.as_ref(), max_edge);
    if mesh.faces.is_empty() {
        return Err("Nothing to emboss".into());
    }
    Ok((mesh, warnings))
}

fn bounds(shape: &[ExPolygon]) -> ([f64; 2], [f64; 2]) {
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in shape.iter().flat_map(|e| e.contour.iter()) {
        let (x, y) = (p.x as f64 * 1e-6, p.y as f64 * 1e-6);
        lo = [lo[0].min(x), lo[1].min(y)];
        hi = [hi[0].max(x), hi[1].max(y)];
    }
    (lo, hi)
}

pub fn mat4(t: Transform) -> Mat4 {
    let r = t.0;
    let col = |c: usize, w: f32| glam::Vec4::new(r[0][c] as f32, r[1][c] as f32, r[2][c] as f32, w);
    Mat4::from_cols(col(0, 0.), col(1, 0.), col(2, 0.), col(3, 1.))
}

/// The text frame on a surface point with outward normal (PrusaSlicer's placement).
pub fn frame_on_surface(point: Vec3, normal: Vec3) -> Mat4 {
    mat4(mica_core::emboss::transform_onto_surface(
        point.to_array().map(f64::from),
        normal.to_array().map(f64::from),
        mica_core::emboss::UP_LIMIT,
    ))
}

/// A 3MF part's emboss source with its frame taken into the GUI object's frame (`placement`:
/// the build transform the import baked into the meshes).
pub fn from_part(part: &mica_core::threemf::Part, placement: Transform) -> Option<Emboss> {
    let (config, frame) = EmbossConfig::from_metadata(&part.metadata)?;
    let frame = frame.unwrap_or_else(|| {
        // No stored frame: the text plane through the part's centre, facing up.
        let (a, b) = part.mesh.bounds();
        let c = [0, 1, 2].map(|i| f64::from(a[i] + b[i]) / 2.);
        mica_core::emboss::transform_onto_surface(c, [0., 0., 1.], mica_core::emboss::UP_LIMIT)
    });
    Some(Emboss {
        config,
        frame: mat4(placement.compose(frame)),
    })
}

/// What an imported object's parts carry for the tool: the emboss of each non-model volume
/// (in the import's volume order), embossed model parts pulled out as editable volumes, and
/// the object's own emboss when its only model part is a text.
#[derive(Default)]
pub struct Imported {
    pub volumes: Vec<Option<Arc<Emboss>>>,
    pub model_volumes: Vec<crate::scene::Volume>,
    pub object: Option<Arc<Emboss>>,
}

/// Takes the emboss sources out of an imported object (see [`Imported`]); the embossed model
/// parts it pulls out leave the regions model, the merged mesh and the part list.
pub fn take_imported(o: &mut mica_core::input::InputObject) -> Imported {
    let mut out = Imported::default();
    let Some((source, placement)) = o.source.clone() else {
        return out;
    };
    let Some(region) = o.region.as_mut() else {
        // A plain object: one model part, which may be the text.
        if let [part] = source.parts.as_slice()
            && part.kind == PartKind::Model
        {
            out.object = from_part(part, placement).map(Arc::new);
        }
        return out;
    };
    // Region volumes are the source parts without unknown kinds, in order.
    let parts: Vec<(usize, &mica_core::threemf::Part)> = source
        .parts
        .iter()
        .enumerate()
        .filter(|(_, p)| !matches!(p.kind, PartKind::Unknown(_)))
        .collect();
    if parts.len() != region.volumes.len() {
        return out;
    }
    let embosses: Vec<Option<Emboss>> =
        parts.iter().map(|(_, p)| from_part(p, placement)).collect();
    out.volumes = region
        .volumes
        .iter()
        .zip(&embosses)
        .filter(|(v, _)| v.kind != PartKind::Model)
        .map(|(_, e)| e.clone().map(Arc::new))
        .collect();
    let models: Vec<usize> = (0..region.volumes.len())
        .filter(|&i| region.volumes[i].kind == PartKind::Model)
        .collect();
    let embossed: Vec<usize> = models
        .iter()
        .copied()
        .filter(|&i| embosses[i].is_some())
        .collect();
    if models.len() == 1 && embossed.len() == 1 {
        out.object = embosses[embossed[0]].clone().map(Arc::new);
    } else if !embossed.is_empty() && embossed.len() < models.len() {
        for &i in embossed.iter().rev() {
            let v = region.volumes.remove(i);
            let (src_index, part) = parts[i];
            let mut vol = crate::scene::Volume::new(part.name.clone(), PartKind::Model, v.mesh);
            vol.emboss = embosses[i].clone().map(Arc::new);
            out.model_volumes.insert(0, vol);
            if src_index < o.parts.len() {
                o.parts.remove(src_index);
            }
        }
        let remaining: Vec<&TriangleMesh> = region
            .volumes
            .iter()
            .filter(|v| v.kind == PartKind::Model)
            .map(|v| &v.mesh)
            .collect();
        o.mesh = mica_core::mesh_edit::merge(&remaining);
    }
    out
}

/// The emboss of a GUI volume as 3MF part metadata, with the frame in the part's coordinates
/// (`to_part`: GUI mesh coordinates → part coordinates).
pub fn metadata(e: &Emboss, to_part: Mat4) -> mica_core::threemf::Settings {
    e.config
        .to_metadata(crate::scene::to_threemf(to_part * e.frame))
}

/// Takes a part's stored emboss frame through `t` (used when a save moves a part's mesh into
/// another frame).
pub fn transform_metadata(m: &mut mica_core::threemf::Settings, t: Transform) {
    let key = format!("{}transform", mica_core::emboss::config::SHAPE_PREFIX);
    if let Some(v) = m.get(&key)
        && let Some(f) = mica_core::emboss::config::parse_transform(v)
    {
        m.insert(
            key,
            mica_core::emboss::config::format_transform(t.compose(f)),
        );
    }
}

/// Tool state while the card is open.
#[derive(Clone, Debug, PartialEq)]
pub struct EmbossTool {
    /// The object being edited (`Object::id`) and its embossed volume (`None`: the object is
    /// a text object itself).
    pub object_id: u64,
    pub volume: Option<u64>,
    /// Why the last rebuild failed, and what it warned about.
    pub error: Option<String>,
    pub warnings: Vec<String>,
    /// Filter for the font list.
    pub font_filter: String,
    /// The text field is being typed in (its undo step is already recorded).
    pub typing: bool,
    /// What Cancel (Esc) puts back: the text as it was when the card opened on it, or `None`
    /// when the card added it (Cancel takes it away again).
    pub original: Option<Original>,
    /// A mesh rebuild running in the background (a text wrapped onto a detailed surface takes
    /// a while; flat text is rebuilt at once).
    pub rebuild: Option<Rebuild>,
}

impl EmbossTool {
    pub fn new(object_id: u64, volume: Option<u64>, original: Option<Original>) -> Self {
        Self {
            object_id,
            volume,
            error: None,
            warnings: vec![],
            font_filter: String::new(),
            typing: false,
            original,
            rebuild: None,
        }
    }
}

/// The edited text before the card opened on it.
#[derive(Clone, Debug, PartialEq)]
pub enum Original {
    Volume(Box<crate::scene::Volume>),
    Object(Box<crate::scene::Object>),
}

/// A built mesh in the frame of the mesh it replaces, and its warnings.
pub type Built = Result<(TriangleMesh, Vec<String>), String>;

/// The mesh of `e` in the frame of the mesh it replaces; `surface` is the object's mesh with
/// its transform into the text frame (for wrapping).
pub fn build_in_frame(
    e: &Emboss,
    kind: Kind,
    surface: Option<&(Arc<TriangleMesh>, Transform)>,
) -> Built {
    let surface: Vec<(&TriangleMesh, Transform)> =
        surface.iter().map(|(m, t)| (m.as_ref(), *t)).collect();
    build(&e.config, kind, &surface).map(|(m, w)| (crate::scene::transform_mesh(&m, e.frame), w))
}

/// A rebuild on a background thread: the source it builds, and its result once done.
#[derive(Clone)]
pub struct Rebuild {
    pub source: Arc<Emboss>,
    result: Arc<std::sync::Mutex<Option<Built>>>,
}

impl Rebuild {
    pub fn spawn(
        source: Arc<Emboss>,
        kind: Kind,
        surface: Option<(Arc<TriangleMesh>, Transform)>,
    ) -> Self {
        let result = Arc::new(std::sync::Mutex::new(None));
        let (out, e) = (result.clone(), source.clone());
        let work = move || {
            let built = build_in_frame(&e, kind, surface.as_ref());
            if let Ok(mut r) = out.lock() {
                *r = Some(built);
            }
        };
        let spawned = std::thread::Builder::new()
            .name("text mesh".into())
            .spawn(work);
        if let Err(e) = spawned
            && let Ok(mut r) = result.lock()
        {
            // No thread: say so on the card instead of waiting forever.
            *r = Some(Err(format!("Cannot rebuild the text: {e}")));
        }
        Self { source, result }
    }

    /// The result once the rebuild is done (taken: later calls return `None`).
    pub fn take(&self) -> Option<Built> {
        self.result.lock().ok()?.take()
    }
}

impl PartialEq for Rebuild {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.result, &other.result)
    }
}

impl std::fmt::Debug for Rebuild {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rebuild").finish_non_exhaustive()
    }
}

/// A fresh text config in the default font.
pub fn default_text() -> EmbossConfig {
    let lib = fonts::library();
    let face = fonts::default_face(&lib);
    EmbossConfig {
        source: EmbossSource::Text(mica_core::emboss::TextSource {
            text: "Text".into(),
            family: face.family.clone(),
            face: face.face.clone(),
            file_name: face.file_name(),
            size_mm: 10.,
            ..Default::default()
        }),
        depth: 1.,
        use_surface: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mica_core::mesh_cut::is_watertight;

    #[test]
    fn text_mesh_is_watertight_in_every_kind_and_on_a_surface() {
        let mut config = default_text();
        if let EmbossSource::Text(t) = &mut config.source {
            t.text = "Mica 3D\nÅgé%&@".into();
        }
        for kind in [Kind::Raised, Kind::Engraved, Kind::Modifier, Kind::Object] {
            let (mesh, warnings) = build(&config, kind, &[]).unwrap();
            assert!(warnings.is_empty(), "{warnings:?}");
            assert!(is_watertight(&mesh), "{kind:?}");
            assert!(mesh.volume() > 0., "{kind:?}");
            let (min, max) = mesh.bounds();
            if kind.outside() {
                assert!(min[2] < 0. && (max[2] - 0.985).abs() < 1e-4);
            } else {
                assert!(max[2] > 0. && (min[2] + 0.985).abs() < 1e-4);
            }
        }
        // Wrapped on a 30 mm sphere's top.
        let sphere = mica_core::mesh_edit::sphere(30., 48, 96);
        let to_frame = Transform([[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., -30.]]);
        config.use_surface = true;
        let (mesh, _) = build(&config, Kind::Raised, &[(&sphere, to_frame)]).unwrap();
        assert!(is_watertight(&mesh));
        let (min, _) = mesh.bounds();
        assert!(min[2] < -0.3, "follows the curve: {min:?}");
    }

    #[test]
    fn svg_mesh_is_watertight() {
        let config = EmbossConfig {
            source: EmbossSource::Svg(mica_core::emboss::SvgSource {
                path: String::new(),
                path_in_3mf: "3D/star.svg".into(),
                data: Arc::from(
                    r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><path d="M50 5 L61 39 L97 39 L68 61 L79 95 L50 74 L21 95 L32 61 L3 39 L39 39 Z"/><circle cx="50" cy="50" r="8" fill="white"/></svg>"#,
                ),
                mm_per_unit: 0.3,
                mirror: [false, false],
                tolerance_mm: 0.1,
            }),
            depth: 2.,
            use_surface: false,
        };
        let (mesh, _) = build(&config, Kind::Engraved, &[]).unwrap();
        assert!(is_watertight(&mesh));
        let (min, max) = mesh.bounds();
        assert!((max[0] - min[0] - 28.2).abs() < 0.2, "{min:?} {max:?}");
    }
}
